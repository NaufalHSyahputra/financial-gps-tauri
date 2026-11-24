use core_engine::{
    domain::{Clock, CurrencyCode, CurrencyScale, Money, NORMALIZE_SQL_FUNCTION, normalize},
    importers::decimal_minor,
    migrations::{INITIAL_SQL, INITIAL_VERSION, checksum},
};

#[test]
fn v1_migration_is_byte_identical() {
    assert_eq!(INITIAL_VERSION, 1);
    assert_eq!(
        checksum(INITIAL_SQL),
        "332a44cc9ab4c71f825106a217d8e69563a4708d655ac8984ab820c9e94b576d"
    );
}

#[test]
fn search_normalization_handles_unicode_case_and_spacing() {
    assert_eq!(NORMALIZE_SQL_FUNCTION, "financial_normalize");
    assert_eq!(normalize("  Café\tStraße\u{a0}ＦＯＯ "), "cafe strasse foo");
    assert_eq!(normalize("CAFE\u{301}"), normalize("Café"));
}

#[test]
fn amounts_are_exact_across_minor_unit_scales() {
    for (currency, scale, major, minor) in [
        (
            "JPY",
            CurrencyScale::Zero,
            "9007199254740993",
            9_007_199_254_740_993,
        ),
        ("USD", CurrencyScale::Two, "-23.45", -2345),
        ("KWD", CurrencyScale::Three, "1.234", 1234),
    ] {
        let currency = CurrencyCode::parse(currency).unwrap();
        let money = Money::from_major(currency, scale, major).unwrap();
        assert_eq!(money.minor, minor);
        assert_eq!(money.to_major(scale).to_string(), major);
    }
    assert!(
        Money::from_major(
            CurrencyCode::parse("USD").unwrap(),
            CurrencyScale::Two,
            "1.001"
        )
        .is_err()
    );
    assert_eq!(
        decimal_minor("9.007,12", 2, ',', Some('.')).unwrap(),
        900_712
    );
    assert!(decimal_minor("0,001", 2, ',', Some('.')).is_err());
}

#[test]
fn ledger_timezone_changes_the_business_date_at_local_midnight() {
    struct FixedClock(chrono::DateTime<chrono::Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            self.0
        }
    }

    let clock = FixedClock("2025-12-31T17:00:00Z".parse().unwrap());
    assert_eq!(
        clock.today(Some(chrono_tz::Asia::Jakarta)).to_string(),
        "2026-01-01"
    );
    assert_eq!(
        clock.today(Some(chrono_tz::America::New_York)).to_string(),
        "2025-12-31"
    );
}
