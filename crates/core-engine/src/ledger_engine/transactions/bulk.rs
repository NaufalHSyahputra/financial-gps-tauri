use super::{self as tx, *};
use crate::ledger_engine::workflows::{
    BulkAction, BulkResult, IdsOrFilter, MutationPreview, Plan, PreviewItem, PreviewStore,
    assign_group, eligible, group_self, notes_after, validated_tag,
};
use std::collections::HashSet;

pub fn bulk_preview(
    ctx: &Context<'_>,
    store: &PreviewStore,
    window: &str,
    selection: &IdsOrFilter,
    action: &BulkAction,
) -> Result<MutationPreview> {
    match action {
        BulkAction::Categorize {
            category_id: Some(id),
        } => tx::assignable_category(ctx, id)?,
        BulkAction::AddTag { tag } | BulkAction::RemoveTag { tag } => {
            validated_tag(tag)?;
        }
        BulkAction::Flag { flagged, due_date } => {
            date_option(due_date.as_deref())?;
            if !*flagged && due_date.is_some() {
                return Err(invalid("dueDate", "flag_required"));
            }
        }
        BulkAction::AssignGroup { group_id } => {
            group_self(ctx, group_id)?;
        }
        _ => {}
    }
    let ids = match selection {
        IdsOrFilter::Ids { ids } => {
            if ids.is_empty()
                || ids.len() > 5000
                || ids.iter().collect::<HashSet<_>>().len() != ids.len()
            {
                return Err(invalid("selection", "invalid_bulk_selection"));
            }
            ids.clone()
        }
        IdsOrFilter::Filter { filter } => {
            let mut ids = Vec::new();
            let mut input = PageReq {
                cursor: None,
                limit: 200,
            };
            loop {
                let page = tx::list(ctx, filter, &input, false)?;
                ids.extend(page.rows.into_iter().map(|row| row.id));
                if ids.len() > 5000 {
                    return Err(invalid("selection", "bulk_selection_too_large"));
                }
                if page.next_cursor.is_none() {
                    break;
                }
                input.cursor = page.next_cursor;
            }
            ids
        }
    };
    let mut selected = Vec::new();
    let mut skipped = Vec::new();
    let mut rows = Vec::new();
    for id in ids {
        let detail = tx::get(ctx, &id)?;
        let blocked = match eligible(ctx, &detail, action) {
            Ok(()) => {
                selected.push(VersionedId {
                    id: id.clone(),
                    version: detail.row.version,
                });
                None
            }
            Err(error) if matches!(error.code, ErrorCode::Validation | ErrorCode::Conflict) => {
                skipped.push(id.clone());
                Some(error.message_key.to_owned())
            }
            Err(error) => return Err(error),
        };
        rows.push(PreviewItem {
            id,
            description: detail.row.description,
            blocked,
        });
    }
    let requires_confirmation = matches!(action, BulkAction::Delete);
    let (preview_token, revision) = store.put(
        ctx,
        window,
        Plan::Bulk {
            selected,
            skipped,
            action: action.clone(),
        },
    )?;
    Ok(MutationPreview {
        preview_token,
        rows,
        revision,
        requires_confirmation,
    })
}
pub fn bulk_apply(
    ctx: &Context<'_>,
    store: &PreviewStore,
    window: &str,
    token: &str,
    confirmed: bool,
) -> Result<BulkResult> {
    let Plan::Bulk {
        selected,
        skipped,
        action,
    } = store.get(ctx, window, token)?
    else {
        return Err(invalid("previewToken", "preview_purpose_mismatch"));
    };
    if matches!(action, BulkAction::Delete) && !confirmed {
        return Err(invalid("confirm", "confirmation_required"));
    }
    let mut changed_ids = Vec::new();
    for target in selected {
        let item = tx::get(ctx, &target.id)?;
        check_version(&item.row.version, &target)?;
        eligible(ctx, &item, &action)?;
        match &action {
            BulkAction::Categorize { category_id } => {
                if let Some(id) = category_id {
                    tx::assignable_category(ctx, id)?;
                }
                ctx.tx.execute(
                    "UPDATE transactions SET category_id=?2,version=version+1 WHERE id=?1",
                    params![target.id, category_id],
                )?;
                tx::mark_override(ctx, &target.id, "category_id")?;
            }
            BulkAction::AddTag { tag } | BulkAction::RemoveTag { tag } => {
                let notes = notes_after(
                    &item.draft.notes,
                    tag,
                    matches!(action, BulkAction::AddTag { .. }),
                )?;
                ctx.tx.execute(
                    "UPDATE transactions SET notes=?2,version=version+1 WHERE id=?1",
                    params![target.id, notes],
                )?;
                tx::mark_override(ctx, &target.id, "notes")?;
                tx::project(ctx, &target.id)?;
            }
            BulkAction::Flag { flagged, due_date } => {
                ctx.tx.execute("UPDATE transactions SET flagged=?2,flag_due_date=?3,version=version+1 WHERE id=?1",params![target.id,flagged,due_date])?;
                tx::mark_override(ctx, &target.id, "flagged")?;
                tx::mark_override(ctx, &target.id, "flag_due_date")?;
            }
            BulkAction::AssignGroup { group_id } => {
                assign_group(ctx, &item, group_id)?;
            }
            BulkAction::Delete => {
                tx::remove(ctx, &target.id, false)?;
            }
        }
        changed_ids.push(target.id);
    }
    Ok(BulkResult {
        changed_ids,
        skipped_ids: skipped,
    })
}
