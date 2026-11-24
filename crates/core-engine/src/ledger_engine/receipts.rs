use std::collections::BTreeMap;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{
    Context, Deleted, Write, accounts, catalog, codec_error, fx, parse_date, revision,
    transactions, transfers, workflows,
};
use crate::domain::{AppError, ErrorCode, Result};

/// The durable receipt holds references and reconciliation data, without copying financial records.
pub trait ReceiptValue: Sized {
    type Reference: Serialize + DeserializeOwned;
    const NEEDS_SAME_REVISION: bool = true;

    fn reference(&self, ctx: &Context<'_>) -> Self::Reference;
    fn reload(ctx: &Context<'_>, reference: Self::Reference) -> Result<Self>;
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt<R> {
    revision: String,
    affected: Vec<String>,
    reference: R,
}

pub(super) fn encode<T: ReceiptValue>(ctx: &Context<'_>, result: &Write<T>) -> Result<String> {
    serde_json::to_string(&Receipt {
        revision: result.revision.clone(),
        affected: result.affected.clone(),
        reference: result.value.reference(ctx),
    })
    .map_err(codec_error)
}

pub(super) fn replay<T: ReceiptValue>(
    ctx: &Context<'_>,
    stored: &str,
    request: &str,
) -> Result<Write<T>> {
    let receipt: Receipt<T::Reference> = serde_json::from_str(stored).map_err(codec_error)?;
    if T::NEEDS_SAME_REVISION && receipt.revision != revision(&ctx.tx)? {
        let mut committed = AppError::new(ErrorCode::StaleRevision, "mutation_already_committed");
        committed.retryable = false;
        committed.fields = Some(BTreeMap::from([
            ("requestId".to_owned(), request.to_owned()),
            ("committedRevision".to_owned(), receipt.revision),
        ]));
        return Err(committed);
    }
    Ok(Write {
        value: T::reload(ctx, receipt.reference)?,
        revision: receipt.revision,
        affected: receipt.affected,
    })
}

macro_rules! stable_result {
    ($($kind:ty),+ $(,)?) => {$(
        impl ReceiptValue for $kind {
            type Reference = Self;
            const NEEDS_SAME_REVISION: bool = false;

            fn reference(&self, _: &Context<'_>) -> Self { self.clone() }

            fn reload(_: &Context<'_>, reference: Self) -> Result<Self> {
                Ok(reference)
            }
        }
    )+};
}

// These results contain only affected record IDs and disposition information.
stable_result!(Deleted, catalog::ReferenceChange, workflows::BulkResult);

macro_rules! record {
    ($kind:ty, |$value:ident| $id:expr, $read:path) => {
        impl ReceiptValue for $kind {
            type Reference = String;

            fn reference(&self, _: &Context<'_>) -> String {
                let $value = self;
                $id.clone()
            }

            fn reload(ctx: &Context<'_>, id: String) -> Result<Self> {
                $read(ctx, &id)
            }
        }
    };
}

record!(
    accounts::StatementSummary,
    |value| value.id,
    accounts::statement_get
);
record!(catalog::CategoryGroup, |value| value.id, catalog::group_get);
record!(catalog::Category, |value| value.id, catalog::category_get);
record!(catalog::Payee, |value| value.id, catalog::payee_get);
record!(fx::FxRate, |value| value.id, fx::get);
record!(
    transactions::TxDetail,
    |value| value.row.id,
    transactions::get
);
record!(transfers::TransferDetail, |value| value.id, transfers::get);

impl ReceiptValue for accounts::AccountDetail {
    type Reference = (String, String);

    fn reference(&self, _: &Context<'_>) -> Self::Reference {
        (self.id.clone(), self.as_of.clone())
    }

    fn reload(ctx: &Context<'_>, (id, as_of): Self::Reference) -> Result<Self> {
        accounts::get(ctx, &id, Some(&as_of))
    }
}

impl ReceiptValue for accounts::CardLineDetail {
    type Reference = (String, String);

    fn reference(&self, ctx: &Context<'_>) -> Self::Reference {
        (self.id.clone(), ctx.today.to_string())
    }

    fn reload(ctx: &Context<'_>, (id, as_of): Self::Reference) -> Result<Self> {
        accounts::line_credit(ctx, &id, parse_date(&as_of)?)
    }
}

impl ReceiptValue for accounts::Settings {
    type Reference = ();

    fn reference(&self, _: &Context<'_>) {}

    fn reload(ctx: &Context<'_>, _: ()) -> Result<Self> {
        accounts::settings(ctx)
    }
}

impl ReceiptValue for transfers::TxPair {
    type Reference = (String, String);

    fn reference(&self, _: &Context<'_>) -> Self::Reference {
        (self.source.row.id.clone(), self.destination.row.id.clone())
    }

    fn reload(ctx: &Context<'_>, (source, destination): Self::Reference) -> Result<Self> {
        Ok(Self {
            source: transactions::get(ctx, &source)?,
            destination: transactions::get(ctx, &destination)?,
        })
    }
}

impl ReceiptValue for workflows::InstallmentDetail {
    type Reference = (String, Vec<String>);

    fn reference(&self, _: &Context<'_>) -> Self::Reference {
        (
            self.series_id.clone(),
            self.transactions.iter().map(|row| row.id.clone()).collect(),
        )
    }

    fn reload(ctx: &Context<'_>, (series_id, ids): Self::Reference) -> Result<Self> {
        let transactions = ids
            .iter()
            .map(|id| transactions::get(ctx, id).map(|item| item.row))
            .collect::<Result<_>>()?;
        Ok(Self {
            series_id,
            transactions,
        })
    }
}

impl ReceiptValue for fx::FxRefreshResult {
    type Reference = Vec<String>;

    fn reference(&self, _: &Context<'_>) -> Self::Reference {
        self.rates.iter().map(|rate| rate.id.clone()).collect()
    }

    fn reload(ctx: &Context<'_>, ids: Self::Reference) -> Result<Self> {
        let rates = ids
            .iter()
            .map(|id| fx::get(ctx, id))
            .collect::<Result<_>>()?;
        Ok(Self { rates })
    }
}
