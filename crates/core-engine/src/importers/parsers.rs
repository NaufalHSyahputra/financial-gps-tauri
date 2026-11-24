use std::io::Read;

use chrono::{DateTime, NaiveDate};
use quick_xml::{Reader, events::Event};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use sha2::{Digest, Sha256};

use crate::{
    domain::{AppError, ErrorCode, Result, normalize, parse_date},
    ledger_engine::{Direction, TxStatus, dto, invalid},
};

const MAX_BYTES: u64 = 20 * 1024 * 1024;
pub(super) const MAX_ROWS: usize = 100_000;
const MAX_FIELD: usize = 4096;

dto! {
    #[derive(Copy, PartialEq, Eq)]
    pub enum StatementFormat { Csv, Ofx, Qif, Camt }

    #[derive(Copy, PartialEq, Eq)]
    pub enum DateStyle { Iso, DayMonthYear, MonthDayYear, YearMonthDay }

    #[derive(Copy)]
    pub enum TextEncoding { Utf8, Latin1, Utf16Le, Utf16Be }

    pub struct CsvMapping {
        #[serde(default)] pub skip_rows: u16,
        #[serde(default = "default_headers")] pub headers: bool,
        pub date_column: u16, pub description_column: u16, pub amount_column: Option<u16>,
        pub inflow_column: Option<u16>, pub outflow_column: Option<u16>, pub id_column: Option<u16>,
        pub notes_column: Option<u16>, pub provider_category_column: Option<u16>,
        pub delimiter: Option<char>, pub decimal_separator: Option<char>, pub group_separator: Option<char>,
        #[serde(default)] pub invert_sign: bool,
    }

    #[derive(Default)]
    pub struct ParserOptions {
        pub date_style: Option<DateStyle>, pub encoding: Option<TextEncoding>,
        pub currency: Option<String>, pub csv: Option<CsvMapping>, pub provider_namespace: Option<String>,
    }

    pub struct ParsedStatementRow {
        pub ordinal: u32, pub bank_date: String, pub direction: Direction, pub amount_minor: String,
        pub currency: String, pub raw_description: String, pub notes: Option<String>,
        pub external_id: Option<String>, pub provider_namespace: Option<String>,
        pub provider_category: Option<String>, pub status: TxStatus,
    }

    pub struct StatementIssue { pub ordinal: u32, pub field: String, pub code: String }

    #[serde(tag="status", content="value")]
    pub enum ParsedRow { Ready(ParsedStatementRow), Error(StatementIssue) }
}

fn default_headers() -> bool {
    true
}

impl Default for CsvMapping {
    fn default() -> Self {
        Self {
            skip_rows: 0,
            headers: true,
            date_column: 0,
            description_column: 1,
            amount_column: Some(2),
            inflow_column: None,
            outflow_column: None,
            id_column: None,
            notes_column: None,
            provider_category_column: None,
            delimiter: None,
            decimal_separator: None,
            group_separator: None,
            invert_sign: false,
        }
    }
}

impl StatementFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Ofx => "ofx",
            Self::Qif => "qif",
            Self::Camt => "camt",
        }
    }

    pub(crate) fn from_sql(value: &str) -> crate::sql::Result<Self> {
        match value {
            "csv" => Ok(Self::Csv),
            "ofx" => Ok(Self::Ofx),
            "qif" => Ok(Self::Qif),
            "camt" => Ok(Self::Camt),
            _ => Err(crate::sql::Error::InvalidQuery),
        }
    }

    pub fn detect(peek: &[u8], extension: Option<&str>) -> Result<Self> {
        for (format, parser) in [
            (Self::Ofx, &OfxParser as &dyn StatementParser),
            (Self::Qif, &QifParser),
            (Self::Camt, &CamtParser),
            (Self::Csv, &CsvParser),
        ] {
            if parser.sniff(peek, extension) {
                return Ok(format);
            }
        }
        Err(invalid("format", "unsupported_statement_format"))
    }

    pub fn parser(self) -> &'static dyn StatementParser {
        match self {
            Self::Csv => &CsvParser,
            Self::Ofx => &OfxParser,
            Self::Qif => &QifParser,
            Self::Camt => &CamtParser,
        }
    }
}

pub struct ParseConfig<'a> {
    pub options: &'a ParserOptions,
    pub currency: &'a str,
    pub scale: u32,
}

pub trait StatementParser {
    fn sniff(&self, peek: &[u8], extension: Option<&str>) -> bool;
    fn parse(
        &self,
        reader: &mut dyn Read,
        config: &ParseConfig<'_>,
        cancelled: &dyn Fn() -> bool,
        emit: &mut dyn FnMut(ParsedRow) -> Result<()>,
    ) -> Result<()>;
}

pub struct CsvParser;
pub struct OfxParser;
pub struct QifParser;
pub struct CamtParser;

fn extension(value: Option<&str>, choices: &[&str]) -> bool {
    value.is_some_and(|value| {
        choices
            .iter()
            .any(|candidate| value.eq_ignore_ascii_case(candidate))
    })
}

pub(super) fn content(
    reader: &mut dyn Read,
    encoding: Option<TextEncoding>,
    latin1_fallback: bool,
) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(crate::domain::io_error)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid("file", "statement_size_limit"));
    }
    let bom = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        Some((TextEncoding::Utf8, 3))
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        Some((TextEncoding::Utf16Le, 2))
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        Some((TextEncoding::Utf16Be, 2))
    } else {
        None
    };
    let (encoding, skip) = bom.unwrap_or((encoding.unwrap_or(TextEncoding::Utf8), 0));
    let bytes = &bytes[skip..];
    let decoded = match encoding {
        TextEncoding::Utf8 => match std::str::from_utf8(bytes) {
            Ok(text) => text.to_owned(),
            Err(_) if latin1_fallback => bytes.iter().map(|byte| char::from(*byte)).collect(),
            Err(_) => return Err(invalid("encoding", "invalid_utf8")),
        },
        TextEncoding::Latin1 => bytes.iter().map(|byte| char::from(*byte)).collect(),
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            if bytes.len() % 2 != 0 {
                return Err(invalid("encoding", "invalid_utf16"));
            }
            let words = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&pair| {
                    if matches!(encoding, TextEncoding::Utf16Le) {
                        u16::from_le_bytes(pair)
                    } else {
                        u16::from_be_bytes(pair)
                    }
                })
                .collect::<Vec<_>>();
            String::from_utf16(&words).map_err(|_| invalid("encoding", "invalid_utf16"))?
        }
    };
    if decoded.len() as u64 > MAX_BYTES || decoded.contains('\0') {
        return Err(invalid("file", "invalid_statement_text"));
    }
    Ok(decoded)
}

fn check(ordinal: usize, cancelled: &dyn Fn() -> bool) -> Result<u32> {
    if cancelled() {
        return Err(AppError::new(ErrorCode::Cancelled, "operation_cancelled"));
    }
    if ordinal > MAX_ROWS {
        return Err(invalid("file", "statement_row_limit"));
    }
    Ok(ordinal as u32)
}

fn entry(ordinal: u32, row: Result<ParsedStatementRow>) -> ParsedRow {
    match row {
        Ok(row) => ParsedRow::Ready(row),
        Err(error) => {
            let (field, code) = error
                .fields
                .and_then(|fields| fields.into_iter().next())
                .unwrap_or_else(|| ("row".to_owned(), error.message_key.to_owned()));
            ParsedRow::Error(StatementIssue {
                ordinal,
                field,
                code,
            })
        }
    }
}

pub fn decimal_minor(value: &str, scale: u32, decimal: char, group: Option<char>) -> Result<i64> {
    let invalid_amount = || invalid("amount", "invalid_localized_amount");
    if value.len() > 96
        || scale > 3
        || decimal.is_ascii_digit()
        || group.is_some_and(|ch| ch == decimal || ch.is_ascii_digit())
    {
        return Err(invalid_amount());
    }
    let raw = value.trim();
    let (negative, raw) = if let Some(magnitude) = raw.strip_prefix('-') {
        (true, magnitude)
    } else if let Some(magnitude) = raw.strip_prefix('+') {
        (false, magnitude)
    } else if let Some(magnitude) = raw.strip_prefix('(').and_then(|v| v.strip_suffix(')')) {
        (true, magnitude)
    } else {
        (false, raw)
    };
    let (major, fraction) = raw
        .split_once(decimal)
        .map_or((raw, None), |(whole, sub)| (whole, Some(sub)));
    let ascii = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let whole = match group.filter(|separator| major.contains(*separator)) {
        Some(separator) => {
            let chunks = major.split(separator).collect::<Vec<_>>();
            if chunks[0].len() > 3
                || !ascii(chunks[0])
                || chunks[1..]
                    .iter()
                    .any(|part| part.len() != 3 || !ascii(part))
            {
                return Err(invalid_amount());
            }
            chunks.concat()
        }
        None if major.is_empty() && fraction.is_some() => String::from("0"),
        None if ascii(major) => major.to_owned(),
        None => return Err(invalid_amount()),
    };
    let canonical = if let Some(fraction) = fraction {
        if !ascii(fraction) {
            return Err(invalid_amount());
        }
        format!("{whole}.{fraction}")
    } else {
        whole
    };
    let value = Decimal::from_str_exact(&canonical)
        .map_err(|_| invalid_amount())?
        .checked_mul(Decimal::from(10u32.pow(scale)))
        .filter(|x| x.is_integer())
        .and_then(|x| x.to_i64())
        .ok_or_else(|| invalid("amount", "amount_exceeds_currency_precision_or_range"))?;
    Ok(if negative { -value } else { value })
}

fn format_date(value: &str, style: DateStyle) -> Result<NaiveDate> {
    let value = value.trim();
    if matches!(style, DateStyle::Iso) {
        return parse_date(value);
    }
    let pieces = value
        .split(['/', '-', '.', '\''])
        .map(str::trim)
        .collect::<Vec<_>>();
    let [a, b, c] = pieces.as_slice() else {
        return Err(invalid("date", "invalid_date"));
    };
    let (year, month, day) = match style {
        DateStyle::DayMonthYear => (*c, *b, *a),
        DateStyle::MonthDayYear => (*c, *a, *b),
        DateStyle::YearMonthDay => (*a, *b, *c),
        DateStyle::Iso => unreachable!(),
    };
    let number = |v: &str| {
        v.parse::<u32>()
            .ok()
            .filter(|_| v.bytes().all(|b| b.is_ascii_digit()))
    };
    let mut y = number(year)
        .filter(|_| matches!(year.len(), 2 | 4))
        .ok_or_else(|| invalid("date", "invalid_date"))?;
    if year.len() == 2 {
        y += if y >= 70 { 1900 } else { 2000 };
    }
    let result = number(month)
        .zip(number(day))
        .and_then(|(m, d)| NaiveDate::from_ymd_opt(y as i32, m, d))
        .ok_or_else(|| invalid("date", "invalid_date"))?;
    parse_date(&result.to_string())
}

struct RowInput<'a> {
    date: NaiveDate,
    amount: i64,
    description: &'a str,
    notes: Option<&'a str>,
    external_id: Option<&'a str>,
    provider_category: Option<&'a str>,
    provider: &'a str,
    status: TxStatus,
}

fn row(ordinal: u32, config: &ParseConfig<'_>, input: RowInput<'_>) -> Result<ParsedStatementRow> {
    let description = input.description.trim();
    if description.is_empty() || description.chars().count() > 1024 {
        return Err(invalid("description", "invalid_description"));
    }
    let magnitude = input.amount.unsigned_abs();
    if !(1..=9_000_000_000_000).contains(&magnitude) {
        return Err(invalid("amount", "posting_amount_out_of_range"));
    }
    let external_id = input.external_id.map(str::trim).filter(|v| !v.is_empty());
    let namespace = config
        .options
        .provider_namespace
        .as_deref()
        .unwrap_or(input.provider);
    if namespace.trim().is_empty()
        || namespace.len() > 128
        || external_id.is_some_and(|id| id.len() > 512 || id.chars().any(char::is_control))
    {
        return Err(invalid("externalId", "invalid_provider_identity"));
    }
    if input
        .notes
        .is_some_and(|notes| notes.chars().count() > 1000)
    {
        return Err(invalid("notes", "notes_too_long"));
    }
    Ok(ParsedStatementRow {
        ordinal,
        bank_date: input.date.to_string(),
        amount_minor: magnitude.to_string(),
        direction: if input.amount < 0 {
            Direction::Debit
        } else {
            Direction::Credit
        },
        currency: config.currency.to_owned(),
        raw_description: description.to_owned(),
        notes: input.notes.map(str::to_owned),
        external_id: external_id.map(str::to_owned),
        provider_namespace: external_id.map(|_| namespace.to_owned()),
        provider_category: input.provider_category.map(str::to_owned),
        status: input.status,
    })
}

fn framed(values: &[&str]) -> String {
    use std::fmt::Write;
    let mut hash = Sha256::new();
    for value in values {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    hash.finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            write!(out, "{byte:02x}").expect("string write");
            out
        })
}

impl ParsedStatementRow {
    pub fn fingerprint(&self, account: &str) -> String {
        framed(&[
            "statement-fingerprint-v1",
            account,
            &self.bank_date,
            &self.currency,
            &self.amount_minor,
            self.direction.as_str(),
            &normalize(&self.raw_description),
        ])
    }

    pub fn external_key(&self, account: &str) -> Option<String> {
        Some(framed(&[
            "statement-external-v1",
            self.provider_namespace.as_deref()?,
            self.external_id.as_deref()?,
            account,
            &self.bank_date,
            &self.currency,
            &self.amount_minor,
            self.direction.as_str(),
        ]))
    }
}

pub fn validate_options(options: &ParserOptions, format: StatementFormat) -> Result<()> {
    if let Some(currency) = &options.currency {
        crate::domain::CurrencyCode::parse(currency)?;
    }
    if options
        .provider_namespace
        .as_ref()
        .is_some_and(|v| v.trim().is_empty() || v.len() > 128 || v.chars().any(char::is_control))
    {
        return Err(invalid("providerNamespace", "invalid_provider_identity"));
    }
    if let Some(mapping) = &options.csv {
        let single = mapping.amount_column.is_some();
        let split = mapping.inflow_column.is_some() && mapping.outflow_column.is_some();
        if format != StatementFormat::Csv
            || single == split
            || (single && (mapping.inflow_column.is_some() || mapping.outflow_column.is_some()))
            || (split && mapping.inflow_column == mapping.outflow_column)
            || mapping
                .delimiter
                .is_some_and(|v| !matches!(v, ',' | ';' | '\t' | '|'))
            || mapping
                .decimal_separator
                .is_some_and(|v| !matches!(v, '.' | ','))
            || mapping
                .group_separator
                .is_some_and(|v| v == mapping.decimal_separator.unwrap_or('.'))
        {
            return Err(invalid("csv", "invalid_csv_mapping"));
        }
    }
    Ok(())
}

impl StatementParser for CsvParser {
    fn sniff(&self, peek: &[u8], ext: Option<&str>) -> bool {
        extension(ext, &["csv", "tsv"])
            || peek
                .split(|byte| *byte == b'\n')
                .next()
                .is_some_and(|line| line.iter().any(|ch| matches!(ch, b',' | b';' | b'\t')))
    }

    fn parse(
        &self,
        reader: &mut dyn Read,
        config: &ParseConfig<'_>,
        cancelled: &dyn Fn() -> bool,
        emit: &mut dyn FnMut(ParsedRow) -> Result<()>,
    ) -> Result<()> {
        validate_options(config.options, StatementFormat::Csv)?;
        let text = content(reader, config.options.encoding, false)?;
        let default = CsvMapping::default();
        let map = config.options.csv.as_ref().unwrap_or(&default);
        let delimiter = map.delimiter.unwrap_or_else(|| {
            let first = text.lines().nth(usize::from(map.skip_rows)).unwrap_or("");
            [',', ';', '\t', '|']
                .into_iter()
                .max_by_key(|candidate| first.matches(*candidate).count())
                .filter(|candidate| first.contains(*candidate))
                .unwrap_or(',')
        });
        let mut records = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .delimiter(delimiter as u8)
            .from_reader(text.as_bytes());
        let decimal = map.decimal_separator.unwrap_or('.');
        let group = map.group_separator;
        let skip = usize::from(map.skip_rows) + usize::from(map.headers);
        for (index, record) in records.records().enumerate().skip(skip) {
            let ordinal = check(index + 1, cancelled)?;
            let parsed = record
                .map_err(|_| invalid("row", "invalid_csv_row"))
                .and_then(|record| {
                    if record.len() > 512 || record.iter().any(|value| value.len() > MAX_FIELD) {
                        return Err(invalid("row", "statement_field_limit"));
                    }
                    let get = |column: u16| {
                        record
                            .get(usize::from(column))
                            .ok_or_else(|| invalid("column", "missing_csv_column"))
                    };
                    let optional = |column: Option<u16>| column.map(get).transpose();
                    let date = format_date(
                        get(map.date_column)?,
                        config.options.date_style.unwrap_or(DateStyle::Iso),
                    )?;
                    let amount = if let Some(column) = map.amount_column {
                        decimal_minor(get(column)?, config.scale, decimal, group)?
                    } else {
                        let read = |column: Option<u16>| -> Result<i64> {
                            let value = optional(column)?.unwrap_or("");
                            if value.trim().is_empty() {
                                return Ok(0);
                            }
                            decimal_minor(value, config.scale, decimal, group)
                        };
                        let incoming = read(map.inflow_column)?;
                        let outgoing = read(map.outflow_column)?;
                        if incoming < 0 || outgoing < 0 || (incoming > 0 && outgoing > 0) {
                            return Err(invalid("amount", "ambiguous_debit_credit_columns"));
                        }
                        incoming - outgoing
                    };
                    row(
                        ordinal,
                        config,
                        RowInput {
                            date,
                            amount: if map.invert_sign { -amount } else { amount },
                            description: get(map.description_column)?,
                            notes: optional(map.notes_column)?,
                            external_id: optional(map.id_column)?,
                            provider_category: optional(map.provider_category_column)?,
                            provider: "csv",
                            status: TxStatus::Posted,
                        },
                    )
                });
            emit(entry(ordinal, parsed))?;
        }
        Ok(())
    }
}

impl StatementParser for OfxParser {
    fn sniff(&self, peek: &[u8], ext: Option<&str>) -> bool {
        extension(ext, &["ofx", "qfx"])
            || String::from_utf8_lossy(peek)
                .to_ascii_uppercase()
                .contains("<OFX")
    }

    fn parse(
        &self,
        reader: &mut dyn Read,
        config: &ParseConfig<'_>,
        cancelled: &dyn Fn() -> bool,
        emit: &mut dyn FnMut(ParsedRow) -> Result<()>,
    ) -> Result<()> {
        let text = content(reader, config.options.encoding, true)?;
        let uppercase = text.to_ascii_uppercase();
        if !uppercase.contains("<OFX")
            || uppercase.contains("<!DOCTYPE")
            || uppercase.contains("<!ENTITY")
        {
            return Err(invalid("file", "invalid_ofx_document"));
        }
        let mut offset = 0;
        for (index, start) in uppercase
            .match_indices("<STMTTRN>")
            .map(|(index, _)| index)
            .enumerate()
        {
            if start < offset {
                continue;
            }
            let ordinal = check(index + 1, cancelled)?;
            let start = start + "<STMTTRN>".len();
            let next = &uppercase[start..];
            let end = ["</STMTTRN>", "<STMTTRN>", "</BANKTRANLIST>"]
                .into_iter()
                .filter_map(|tag| next.find(tag))
                .min()
                .map(|relative| start + relative)
                .unwrap_or(text.len());
            offset = end;
            let find = |tag: &str| -> Result<Option<String>> {
                let tag = format!("<{tag}>");
                let Some(position) = uppercase[start..end].find(&tag) else {
                    return Ok(None);
                };
                let value = &text[start + position + tag.len()..end];
                let value = value.split(['<', '\r', '\n']).next().unwrap_or("").trim();
                if value.len() > MAX_FIELD {
                    return Err(invalid("row", "statement_field_limit"));
                }
                Ok(Some(
                    quick_xml::escape::unescape(value)
                        .map_err(|_| invalid("row", "invalid_ofx_entity"))?
                        .into_owned(),
                ))
            };
            let parsed = (|| {
                let date = find("DTPOSTED")?
                    .or(find("DTUSER")?)
                    .ok_or_else(|| invalid("date", "missing_ofx_date"))?;
                let date = date
                    .get(..8)
                    .and_then(|s| NaiveDate::parse_from_str(s, "%Y%m%d").ok())
                    .ok_or_else(|| invalid("date", "invalid_ofx_date"))?;
                let amount = decimal_minor(
                    &find("TRNAMT")?.ok_or_else(|| invalid("amount", "missing_ofx_amount"))?,
                    config.scale,
                    '.',
                    None,
                )?;
                let notes = find("MEMO")?;
                let name = find("NAME")?;
                let external_id = find("FITID")?;
                let category = find("SIC")?;
                row(
                    ordinal,
                    config,
                    RowInput {
                        date,
                        amount,
                        description: name.as_deref().or(notes.as_deref()).unwrap_or(""),
                        notes: notes.as_deref(),
                        external_id: external_id.as_deref(),
                        provider_category: category.as_deref(),
                        provider: "ofx",
                        status: TxStatus::Posted,
                    },
                )
            })();
            emit(entry(ordinal, parsed))?;
        }
        Ok(())
    }
}

impl StatementParser for QifParser {
    fn sniff(&self, peek: &[u8], ext: Option<&str>) -> bool {
        extension(ext, &["qif"])
            || String::from_utf8_lossy(peek)
                .trim_start_matches('\u{feff}')
                .trim_start()
                .starts_with("!Type:")
    }
    fn parse(
        &self,
        reader: &mut dyn Read,
        config: &ParseConfig<'_>,
        cancelled: &dyn Fn() -> bool,
        emit: &mut dyn FnMut(ParsedRow) -> Result<()>,
    ) -> Result<()> {
        let text = content(reader, config.options.encoding, false)?;
        if !text.trim_start().starts_with("!Type:") {
            return Err(invalid("file", "invalid_qif_document"));
        }
        let split = text
            .split_inclusive('^')
            .filter(|record| {
                record.lines().any(|line| {
                    let line = line.trim();
                    !line.is_empty() && !line.starts_with('!') && line != "^"
                })
            })
            .collect::<Vec<_>>();
        check(split.len(), cancelled)?;
        let styles = [
            DateStyle::Iso,
            DateStyle::DayMonthYear,
            DateStyle::MonthDayYear,
            DateStyle::YearMonthDay,
        ];
        let candidates = styles
            .into_iter()
            .filter(|style| {
                config
                    .options
                    .date_style
                    .is_some_and(|selected| selected == *style)
                    || (config.options.date_style.is_none()
                        && split
                            .iter()
                            .filter_map(|r| qif_field(r, 'D'))
                            .filter(|date| {
                                styles.iter().any(|style| format_date(date, *style).is_ok())
                            })
                            .all(|date| format_date(date, *style).is_ok()))
            })
            .collect::<Vec<_>>();
        for (index, record) in split.into_iter().enumerate() {
            let ordinal = check(index + 1, cancelled)?;
            let parsed = (|| {
                if record.len() > MAX_FIELD * 4 {
                    return Err(invalid("row", "statement_field_limit"));
                }
                let date =
                    qif_field(record, 'D').ok_or_else(|| invalid("date", "missing_qif_date"))?;
                let mut dates = candidates
                    .iter()
                    .filter_map(|style| format_date(date, *style).ok())
                    .collect::<Vec<_>>();
                dates.sort_unstable();
                dates.dedup();
                let date = match dates.as_slice() {
                    [one] => *one,
                    [] => return Err(invalid("date", "invalid_qif_date")),
                    _ => return Err(invalid("date", "ambiguous_qif_date_style")),
                };
                let amount = decimal_minor(
                    qif_field(record, 'T')
                        .ok_or_else(|| invalid("amount", "missing_qif_amount"))?,
                    config.scale,
                    '.',
                    Some(','),
                )?;
                row(
                    ordinal,
                    config,
                    RowInput {
                        date,
                        amount,
                        description: qif_field(record, 'P')
                            .or(qif_field(record, 'M'))
                            .unwrap_or(""),
                        notes: qif_field(record, 'M'),
                        external_id: None,
                        provider_category: qif_field(record, 'L'),
                        provider: "qif",
                        status: TxStatus::Posted,
                    },
                )
            })();
            emit(entry(ordinal, parsed))?;
        }
        Ok(())
    }
}
fn qif_field(record: &str, field: char) -> Option<&str> {
    record
        .lines()
        .find_map(|line| line.trim_start().strip_prefix(field))
        .map(str::trim)
}

#[derive(Default)]
struct CamtEntry {
    amount: Option<String>,
    currency: Option<String>,
    direction: Option<String>,
    date: Option<String>,
    id: Option<String>,
    description: Option<String>,
    status: Option<String>,
    details: usize,
}
impl StatementParser for CamtParser {
    fn sniff(&self, peek: &[u8], ext: Option<&str>) -> bool {
        extension(ext, &["camt"]) || {
            let peek = String::from_utf8_lossy(peek);
            peek.contains("camt.05") || peek.contains("<BkToCstmrStmt")
        }
    }

    fn parse(
        &self,
        reader: &mut dyn Read,
        config: &ParseConfig<'_>,
        cancelled: &dyn Fn() -> bool,
        emit: &mut dyn FnMut(ParsedRow) -> Result<()>,
    ) -> Result<()> {
        let text = content(reader, config.options.encoding, false)?;
        let mut xml = Reader::from_str(&text);
        xml.config_mut().trim_text(true);
        let mut path: Vec<Vec<u8>> = Vec::new();
        let mut root_seen = false;
        let mut active = None::<CamtEntry>;
        let mut ordinal = 0;
        loop {
            if cancelled() {
                return Err(AppError::new(ErrorCode::Cancelled, "operation_cancelled"));
            }
            match xml
                .read_event()
                .map_err(|_| invalid("file", "invalid_camt_xml"))?
            {
                Event::Start(tag) => {
                    let local = tag.local_name().as_ref().as_bytes().to_vec();
                    if path.len() >= 48 || local.len() > 128 {
                        return Err(invalid("file", "xml_depth_limit"));
                    }
                    if matches!(
                        local.as_slice(),
                        b"BkToCstmrStmt" | b"BkToCstmrDbtCdtNtfctn" | b"BkToCstmrAcctRpt"
                    ) {
                        root_seen = true;
                    }
                    if local == b"Ntry" && root_seen {
                        if active.is_some() {
                            return Err(invalid("file", "nested_camt_entry"));
                        }
                        active = Some(CamtEntry::default());
                    }
                    if let Some(item) = &mut active {
                        if local == b"TxDtls" {
                            item.details += 1;
                        }
                        if local == b"Amt" && path.last().is_some_and(|tag| tag == b"Ntry") {
                            for attr in tag.attributes() {
                                let attr = attr.map_err(|_| invalid("file", "invalid_camt_xml"))?;
                                if attr.key.local_name().as_ref() == "Ccy" {
                                    item.currency = Some(
                                        attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                            .map_err(|_| {
                                                invalid("currency", "invalid_camt_currency")
                                            })?
                                            .into_owned(),
                                    );
                                }
                            }
                        }
                    }
                    path.push(local);
                }
                Event::End(tag) => {
                    if tag.local_name().as_ref() == "Ntry"
                        && let Some(item) = active.take()
                    {
                        ordinal += 1;
                        let index = check(ordinal, cancelled)?;
                        emit(entry(index, finish_camt(index, config, item)))?;
                    }
                    if path.pop().is_none() {
                        return Err(invalid("file", "invalid_camt_xml"));
                    }
                }
                Event::Text(value) => {
                    let decoded = value.xml_content(quick_xml::XmlVersion::Implicit1_0);
                    if let Some(item) = &mut active {
                        camt_text(item, &path, &decoded);
                    }
                }
                Event::CData(value) => {
                    let decoded = value.as_ref();
                    if let Some(item) = &mut active {
                        camt_text(item, &path, decoded);
                    }
                }
                Event::DocType(_) => return Err(invalid("file", "xml_dtd_forbidden")),
                Event::GeneralRef(reference) => {
                    let encoded = reference.as_ref();
                    let text = format!("&{encoded};");
                    let value = quick_xml::escape::unescape(&text)
                        .map_err(|_| invalid("file", "xml_entity_forbidden"))?;
                    if let Some(item) = &mut active {
                        camt_text(item, &path, &value);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if !root_seen || !path.is_empty() {
            return Err(invalid("file", "invalid_camt_document"));
        }
        Ok(())
    }
}

fn camt_text(item: &mut CamtEntry, path: &[Vec<u8>], value: &str) {
    let Some(tail) = path.last().map(Vec::as_slice) else {
        return;
    };
    let parent = path.iter().rev().nth(1).map(Vec::as_slice);
    let target = match (tail, parent) {
        (b"Amt", Some(b"Ntry")) => &mut item.amount,
        (b"CdtDbtInd", Some(b"Ntry")) => &mut item.direction,
        (b"Dt" | b"DtTm", Some(b"BookgDt")) => &mut item.date,
        (b"AcctSvcrRef" | b"NtryRef", Some(b"Ntry")) => &mut item.id,
        (b"Sts", Some(b"Ntry")) | (b"Cd", Some(b"Sts")) => &mut item.status,
        (b"AddtlNtryInf" | b"Ustrd", _) => &mut item.description,
        _ => return,
    };
    let current = target.get_or_insert_with(String::new);
    let allowed = MAX_FIELD.saturating_add(1).saturating_sub(current.len());
    let boundary = value.floor_char_boundary(value.len().min(allowed));
    current.push_str(&value[..boundary]);
}

fn finish_camt(
    index: u32,
    config: &ParseConfig<'_>,
    item: CamtEntry,
) -> Result<ParsedStatementRow> {
    if item.details > 1 {
        return Err(invalid("row", "camt_entry_contains_multiple_movements"));
    }
    if item
        .currency
        .as_deref()
        .is_some_and(|code| code != config.currency)
    {
        return Err(invalid("currency", "statement_currency_mismatch"));
    }
    let date = item
        .date
        .as_deref()
        .ok_or_else(|| invalid("date", "missing_camt_date"))?;
    let date = if date.contains('T') {
        DateTime::parse_from_rfc3339(date)
            .map_err(|_| invalid("date", "invalid_camt_timestamp"))?
            .date_naive()
    } else {
        parse_date(date)?
    };
    let amount = decimal_minor(
        item.amount
            .as_deref()
            .ok_or_else(|| invalid("amount", "missing_camt_amount"))?,
        config.scale,
        '.',
        None,
    )?;
    if amount <= 0 {
        return Err(invalid("amount", "invalid_camt_amount"));
    }
    let amount = match item.direction.as_deref() {
        Some("DBIT") => -amount,
        Some("CRDT") => amount,
        _ => return Err(invalid("direction", "invalid_camt_direction")),
    };
    let status = match item.status.as_deref() {
        Some("PDNG" | "INFO") => TxStatus::Pending,
        _ => TxStatus::Posted,
    };
    row(
        index,
        config,
        RowInput {
            date,
            amount,
            description: item
                .description
                .as_deref()
                .unwrap_or("Bank statement entry"),
            notes: None,
            external_id: item.id.as_deref(),
            provider_category: None,
            provider: "camt",
            status,
        },
    )
}
