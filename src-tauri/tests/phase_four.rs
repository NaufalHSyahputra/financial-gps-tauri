use financial_gps::{
    crypto::Credential,
    domain::{ErrorCode, Protection, Result, parse_date},
    importers::{self, CsvMapping, ParseConfig, ParsedRow, ParserOptions, StatementFormat},
    ledger_engine::{
        Context, Mutation, PageReq, accounts, money, new_id, staging::Stage, transactions,
    },
    planning::{allocations, assets, loans, recurring},
    rules,
    storage::{CreateLedger, LedgerManager, PathGrants, PathPurpose, attachments, snapshot},
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn dto<T: DeserializeOwned>(json: Value) -> T {
    serde_json::from_value(json).unwrap()
}
fn ready(rows: &[ParsedRow]) -> &importers::ParsedStatementRow {
    assert_eq!(rows.len(), 1);
    match &rows[0] {
        ParsedRow::Ready(row) => row,
        ParsedRow::Error(error) => panic!("{error:?}"),
    }
}
fn parse(
    format: StatementFormat,
    mut input: &[u8],
    options: &ParserOptions,
    currency: &str,
    scale: u32,
) -> Result<Vec<ParsedRow>> {
    let mut rows = Vec::new();
    format.parser().parse(
        &mut input,
        &ParseConfig {
            options,
            currency,
            scale,
        },
        &|| false,
        &mut |row| {
            rows.push(row);
            Ok(())
        },
    )?;
    Ok(rows)
}

#[test]
fn statement_formats_and_localized_csv() {
    let options = ParserOptions {
        csv: Some(CsvMapping {
            delimiter: Some(';'),
            decimal_separator: Some(','),
            group_separator: Some('.'),
            amount_column: None,
            inflow_column: Some(2),
            outflow_column: Some(3),
            ..CsvMapping::default()
        }),
        ..ParserOptions::default()
    };
    let csv = b"\xef\xbb\xbfDate;Description;In;Out\r\n2026-01-01;Coffee;;1.234.567\r\n";
    let rows = parse(StatementFormat::Csv, csv, &options, "IDR", 0).unwrap();
    assert_eq!(ready(&rows).amount_minor, "1234567");
    assert_eq!(ready(&rows).direction.as_str(), "debit");
    assert_eq!(
        importers::decimal_minor("1,234.56", 2, '.', Some(',')).unwrap(),
        123456
    );
    assert!(importers::decimal_minor("1,23.45", 2, '.', Some(',')).is_err());

    let defaults = ParserOptions::default();
    let ofx=b"OFXHEADER:100\n<OFX><BANKTRANLIST><STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20260104<TRNAMT>-7.25<NAME>Caf\xe9</STMTTRN></BANKTRANLIST><LEDGERBAL><BALAMT>10.00</LEDGERBAL></OFX>";
    let rows = parse(StatementFormat::Ofx, ofx, &defaults, "USD", 2).unwrap();
    let ofx = ready(&rows);
    assert_eq!(ofx.amount_minor, "725");
    assert_eq!(ofx.raw_description, "Café");
    assert!(ofx.external_id.is_none());

    let ambiguous = b"!Type:Bank\nD02/03/2026\nT-5.00\nPShop\n^\n";
    let rows = parse(StatementFormat::Qif, ambiguous, &defaults, "USD", 2).unwrap();
    assert!(matches!(&rows[0],ParsedRow::Error(error) if error.code=="ambiguous_qif_date_style"));
    let qif = b"!Type:Bank\nD13/03/2026\nT-5.00\nPShop\n^\n";
    assert_eq!(
        ready(&parse(StatementFormat::Qif, qif, &defaults, "USD", 2).unwrap()).bank_date,
        "2026-03-13"
    );

    let camt=br#"<Document><BkToCstmrStmt><Stmt><Ntry><Amt Ccy="EUR">18.99</Amt><CdtDbtInd>DBIT</CdtDbtInd><BookgDt><Dt>2026-01-27</Dt></BookgDt><NtryRef>entry-1</NtryRef><AddtlNtryInf>Caf&#233;</AddtlNtryInf></Ntry></Stmt></BkToCstmrStmt></Document>"#;
    let rows = parse(StatementFormat::Camt, camt, &defaults, "EUR", 2).unwrap();
    assert_eq!(ready(&rows).amount_minor, "1899");
    assert_eq!(ready(&rows).raw_description, "Café");
    assert!(
        parse(
            StatementFormat::Camt,
            br#"<!DOCTYPE Document SYSTEM "file:///does-not-exist"><Document/>"#,
            &defaults,
            "EUR",
            2
        )
        .is_err()
    );
}
#[test]
fn rule_ast_enforces_shape_and_regex_bounds() {
    let rule = |value: &str| {
        dto::<rules::RuleDraft>(json!({
            "name":"Coffee","priority":10,"enabled":true,"applyExisting":true,"overwriteExistingCategory":false,
            "schemaVersion":1,"conditions":{"op":"and","children":[{"kind":"predicate","value":{
                "field":"description","operator":"regex","value":{"type":"text","value":value}}}]},
            "actions":[{"kind":"appendNotes","value":"Coffee"}]
        }))
    };
    assert!(rules::CompiledRule::new(&rule("^cafe$")).is_ok());
    for invalid in ["", "(", "a*", "(?=x)", "a{1000000}"] {
        assert!(
            rules::CompiledRule::new(&rule(invalid)).is_err(),
            "{invalid}"
        );
    }
    assert!(rules::CompiledRule::new(&rule(&"a".repeat(4096))).is_err());
    let mut empty = rule("cafe");
    empty.conditions.children.clear();
    assert!(rules::CompiledRule::new(&empty).is_err());
    let mut nested = serde_json::to_value(rule("cafe")).unwrap();
    nested["conditions"]["children"][0] = json!({"kind":"group","value":{"op":"or","children":[
        {"kind":"group","value":{"op":"and","children":[]}}
    ]}});
    assert!(serde_json::from_value::<rules::RuleDraft>(nested).is_err());
}
#[test]
fn calendar_clamps_then_shifts_and_amortization_conserves_principal() {
    let rule = |policy: &str| {
        dto::<recurring::RecurringDraft>(json!({
            "accountId":"a","amount":{"currency":"USD","minor":"100"},"direction":"debit","description":"Rent",
            "frequency":"monthly","anchorDay":31,"startsOn":"2024-01-31","weekendPolicy":policy,"active":true
        }))
    };
    let start = parse_date("2024-01-31").unwrap();
    let end = parse_date("2024-04-02").unwrap();
    let dates = recurring::dates(&rule("nextMonday"), start, end).unwrap();
    assert_eq!(
        dates
            .iter()
            .map(|(nominal, scheduled)| (nominal.to_string(), scheduled.to_string()))
            .collect::<Vec<_>>(),
        [
            ("2024-01-31".into(), "2024-01-31".into()),
            ("2024-02-29".into(), "2024-02-29".into()),
            ("2024-03-31".into(), "2024-04-01".into())
        ]
    );
    let dates = recurring::dates(&rule("previousFriday"), start, end).unwrap();
    assert_eq!(dates.last().unwrap().1.to_string(), "2024-03-29");
    for amortization in ["price", "sac"] {
        let terms = dto::<loans::LoanTerms>(
            json!({"accountId":"a","principal":{"currency":"USD","minor":"100"},
            "interestRatePercent":"0","ratePeriod":"yearlyEffective","amortization":amortization,
            "kind":"personal","termMonths":3,"firstPaymentDate":"2024-01-31"}),
        );
        let schedule = loans::calculate(&terms, start).unwrap();
        assert_eq!(
            schedule
                .rows
                .iter()
                .map(|row| row.principal.minor.as_str())
                .collect::<Vec<_>>(),
            ["33", "33", "34"]
        );
        assert_eq!(schedule.rows[1].due_date, "2024-02-29");
        assert_eq!(schedule.rows[2].balance.minor, "0");
    }
}
#[test]
fn weighted_assets_apply_splits_first_and_allocate_final_residue() {
    let trade = |id: &str, date: &str, trade: Value| {
        dto::<assets::Trade>(json!({
            "id":id,"version":"1","createdAt":"2026-01-01T00:00:00Z",
            "draft":{"assetId":"asset","tradedOn":date,"trade":trade}
        }))
    };
    let asset=assets::replay("USD",&[
        trade("a","2026-01-01",json!({"kind":"buy","units":"5","gross":money("USD",1000),"fees":money("USD",100)})),
        trade("b","2026-02-01",json!({"kind":"sell","units":"7","gross":money("USD",2000),"fees":money("USD",100)})),
        trade("c","2026-02-01",json!({"kind":"split","oldUnits":"1","newUnits":"2"})),
    ],parse_date("2026-02-01").unwrap()).unwrap();
    assert_eq!(asset.units, "3");
    assert_eq!(asset.cost_basis.minor, "330");
    assert_eq!(asset.realized.minor, "1130");
    let shares = allocations::allocate(
        2,
        &allocations::Method::Equal,
        &["a", "b", "c", "d"].map(|member| allocations::ShareInput {
            member_id: member.to_owned(),
            value: None,
        }),
    )
    .unwrap();
    assert_eq!(
        shares
            .iter()
            .map(|share| share.minor.as_str())
            .collect::<Vec<_>>(),
        ["1", "1", "0", "0"]
    );
}

fn credential(password: &str) -> Credential {
    dto(json!(password))
}
fn draft(name: &str, encrypted: bool) -> CreateLedger {
    CreateLedger {
        name: name.to_owned(),
        destination: None,
        base_currency: "USD".into(),
        timezone: None,
        protection: if encrypted {
            Protection::Encrypted
        } else {
            Protection::Plain
        },
        credential: encrypted.then(|| credential("test SQLCipher passphrase")),
        remember_key: false,
    }
}
fn account(ctx: &Context<'_>) -> Result<String> {
    Ok(accounts::save(
        ctx,
        None,
        &dto(json!({"name":"Checking","kind":"checking","currencyCode":"USD"})),
    )?
    .id)
}
fn tx_draft(account: &str) -> transactions::TxDraft {
    dto(
        json!({"accountId":account,"bankDate":"2026-01-15","amount":money("USD",1000),"direction":"debit","status":"posted",
        "description":"Straße Cafe","notes":"Fixture note"}),
    )
}
fn page() -> PageReq {
    PageReq {
        cursor: None,
        limit: 50,
    }
}

#[test]
fn staged_import_persists_identity_and_blocks_undo_after_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("import", false))
        .unwrap()
        .scope;
    let id = manager
        .write_domain("main", &scope, |tx| {
            account(&Context::system(
                financial_gps::storage::SqliteSession(tx),
                &scope,
            )?)
        })
        .unwrap();
    let store = Stage::default();
    let undo = Stage::default();
    let config = importers::PreviewRequest {
        account_id: id,
        format: Some(StatementFormat::Ofx),
        template_id: None,
        parser_options: None,
        detect_duplicates: None,
    };
    let content=b"<OFX><BANKTRANLIST><STMTTRN><DTPOSTED>20260115<TRNAMT>-10.00<FITID>one<NAME>Cafe</STMTTRN></BANKTRANLIST></OFX>";
    let preview = || {
        manager
            .read_domain("main", &scope, |tx| {
                let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
                importers::preview(&ctx, &store, "statement.ofx", content, &config, &|| false)
            })
            .unwrap()
    };
    let first = preview();
    assert_eq!(first.ready, 1);
    let batch = manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            importers::commit(&ctx, &store, &first.preview_token, &[], &|| false)
        })
        .unwrap();
    assert_eq!(batch.inserted, 1);
    assert_eq!(preview().duplicate, 1);
    let before = manager
        .read_domain("main", &scope, |tx| {
            importers::undo_preview(
                &Context::system(financial_gps::storage::SqliteSession(tx), &scope)?,
                &undo,
                &batch.id,
                None,
            )
        })
        .unwrap();
    assert!(before.rows[0].blocked.is_none());
    manager.write("main",&scope,|tx|{
        tx.execute("INSERT INTO attachments VALUES(?1,?2,?1,'fixture.pdf','application/pdf',1,?3,'ready',0)",
            rusqlite::params![new_id(),before.rows[0].entity_id,"a".repeat(64)])?;
        Ok(())
    }).unwrap();
    let after = manager
        .read_domain("main", &scope, |tx| {
            importers::undo_preview(
                &Context::system(financial_gps::storage::SqliteSession(tx), &scope)?,
                &undo,
                &batch.id,
                None,
            )
        })
        .unwrap();
    assert!(after.rows[0].blocked.is_some());
}
#[test]
fn encrypted_archive_round_trip_preserves_postings_and_attachments() {
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let original = manager.create("main", draft("original", true)).unwrap();
    let scope = original.scope.clone();
    let posting = manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            let id = account(&ctx)?;
            transactions::save(&ctx, None, &tx_draft(&id))
        })
        .unwrap();
    let file = temp.path().join("receipt.pdf");
    let pdf = b"%PDF-1.7\nFixture";
    std::fs::write(&file, pdf).unwrap();
    let grants = PathGrants::default();
    let grant = grants
        .issue("main", PathPurpose::AttachmentRead, &file)
        .unwrap();
    let mutation = Mutation {
        scope: scope.clone(),
        request_id: new_id(),
        expected_revision: None,
    };
    let result = manager
        .attachment_add(
            "main",
            &mutation,
            &posting.row.id,
            &grant.id,
            || {
                Ok(vec![grants.consume(
                    "main",
                    PathPurpose::AttachmentRead,
                    &grant.id,
                )?])
            },
            || Ok(()),
        )
        .unwrap();
    let attachment = result.value.attachments[0].id.clone();
    assert!(matches!(
        result.value.attachments[0].state,
        attachments::AttachmentState::Ready
    ));
    assert_eq!(
        manager
            .attachment_bytes("main", &scope, &attachment, |_, bytes| Ok(bytes.to_vec()))
            .unwrap(),
        pdf
    );
    let blobs = temp
        .path()
        .join("attachments")
        .join(&scope.ledger_instance_id);
    let raw = std::fs::read(blobs.join(format!("{attachment}.blob"))).unwrap();
    assert!(!raw.windows(pdf.len()).any(|field| field == pdf));

    let (image, objects) = manager.backup_image("main", &scope, &|| false).unwrap();
    let mut bytes = Vec::new();
    let password = credential("independent backup passphrase");
    let manifest = snapshot::write(&image, &objects, &mut bytes, &password, &|| false).unwrap();
    assert_eq!(manifest.attachments[0].id, attachment);
    assert!(
        matches!(snapshot::open(bytes.as_slice(),&credential("wrong"),&||false),Err(error) if error.code==ErrorCode::BadCredentialOrCorrupt)
    );
    let mut tampered = bytes.clone();
    *tampered.last_mut().unwrap() ^= 1;
    assert!(
        matches!(snapshot::open(tampered.as_slice(),&password,&||false),Err(error) if error.code==ErrorCode::BadCredentialOrCorrupt)
    );
    let archive = snapshot::open(bytes.as_slice(), &password, &|| false).unwrap();
    let clean_install = tempfile::tempdir().unwrap();
    let restored_manager = LedgerManager::new(clean_install.path()).unwrap();
    let restored = restored_manager
        .restore_new("main", draft("restored", true), archive)
        .unwrap();
    assert_ne!(original.ledger_uuid, restored.ledger_uuid);
    assert_ne!(scope, restored.scope);
    restored_manager
        .read_domain("main", &restored.scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &restored.scope)?;
            let found = transactions::get(&ctx, &posting.row.id)?;
            assert_eq!(found.row.reporting_date, posting.row.reporting_date);
            assert_eq!(found.row.amount.minor, "1000");
            let matches =
                transactions::list(&ctx, &dto(json!({"text":"strasse"})), &page(), false)?;
            assert_eq!(matches.rows.len(), 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        restored_manager
            .attachment_bytes("main", &restored.scope, &attachment, |_, bytes| Ok(
                bytes.to_vec()
            ))
            .unwrap(),
        pdf
    );
}

#[test]
fn attachment_recovery_adopts_staging_and_finishes_deletions() {
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let original = manager.create("main", draft("attachments", false)).unwrap();
    let scope = original.scope.clone();
    let posting = manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            let id = account(&ctx)?;
            transactions::save(&ctx, None, &tx_draft(&id))
        })
        .unwrap();
    let file = temp.path().join("receipt.pdf");
    let pdf = b"%PDF-1.7\nRecovery";
    std::fs::write(&file, pdf).unwrap();
    let grants = PathGrants::default();
    let grant = grants
        .issue("main", PathPurpose::AttachmentRead, &file)
        .unwrap();
    let mutation = Mutation {
        scope: scope.clone(),
        request_id: new_id(),
        expected_revision: None,
    };
    let result = manager
        .attachment_add(
            "main",
            &mutation,
            &posting.row.id,
            &grant.id,
            || {
                Ok(vec![grants.consume(
                    "main",
                    PathPurpose::AttachmentRead,
                    &grant.id,
                )?])
            },
            || Ok(()),
        )
        .unwrap();
    let id = &result.value.attachments[0].id;
    let root = temp
        .path()
        .join("attachments")
        .join(&scope.ledger_instance_id);
    let partial = root.join(format!("{id}.part"));
    let final_file = root.join(format!("{id}.blob"));
    std::fs::rename(&final_file, &partial).unwrap();
    manager
        .write("main", &scope, |tx| {
            tx.execute("UPDATE attachments SET state='staged' WHERE id=?1", [id])?;
            Ok(())
        })
        .unwrap();
    manager.close("main", &scope).unwrap();
    let reopen = || {
        manager
            .open(
                "main",
                uuid::Uuid::new_v4(),
                scope.ledger_instance_id.clone(),
                || {
                    Ok(financial_gps::storage::OpenTarget::Registered(
                        uuid::Uuid::parse_str(&scope.ledger_instance_id).unwrap(),
                    ))
                },
            )
            .unwrap()
            .session
            .unwrap()
            .scope
    };
    let next = reopen();
    assert!(final_file.is_file());
    assert!(!partial.exists());
    assert_eq!(
        manager
            .attachment_bytes("main", &next, id, |_, bytes| Ok(bytes.to_vec()))
            .unwrap(),
        pdf
    );
    manager
        .write("main", &next, |tx| {
            tx.execute("UPDATE attachments SET state='deleting' WHERE id=?1", [id])?;
            Ok(())
        })
        .unwrap();
    manager.close("main", &next).unwrap();
    let next = reopen();
    assert!(!final_file.exists());
    let remaining = manager
        .read_domain("main", &next, |tx| {
            attachments::list(&financial_gps::storage::sqlite_view(tx), &posting.row.id)
        })
        .unwrap();
    assert!(remaining.is_empty());
}

#[test]
fn plain_wal_backup_and_confirmed_replacement_keep_a_safety_archive() {
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let source = manager
        .create("main", draft("snapshot source", false))
        .unwrap();
    let scope = source.scope.clone();
    let transaction = manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            let id = account(&ctx)?;
            transactions::save(&ctx, None, &tx_draft(&id))
        })
        .unwrap();
    let password = credential("backup for replacement");
    let (image, objects) = manager.backup_image("main", &scope, &|| false).unwrap();
    let mut payload = Vec::new();
    snapshot::write(&image, &objects, &mut payload, &password, &|| false).unwrap();
    let archive = snapshot::open(payload.as_slice(), &password, &|| false).unwrap();
    let target = manager
        .create("main", draft("replacement target", false))
        .unwrap();
    let replaced = manager
        .restore_replace(
            "main",
            &target.scope,
            financial_gps::storage::ReplacementSource {
                expected_revision: &target.revision,
                protection: Protection::Plain,
                credential: None,
                safety_password: &password,
                archive,
            },
            &|| false,
            || Ok(()),
        )
        .unwrap();
    assert_eq!(target.ledger_uuid, replaced.ledger_uuid);
    assert_eq!(
        target.scope.ledger_instance_id,
        replaced.scope.ledger_instance_id
    );
    assert_ne!(target.scope.session_id, replaced.scope.session_id);
    manager
        .read_domain("main", &replaced.scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &replaced.scope)?;
            assert_eq!(
                transactions::get(&ctx, &transaction.row.id)?
                    .row
                    .amount
                    .minor,
                "1000"
            );
            let backups = manager.backup_list("main", &ctx, &page())?;
            assert_eq!(backups.rows.len(), 1);
            let before = manager.backup_file("main", &replaced.scope, &backups.rows[0].id)?;
            let original = snapshot::open(before, &password, &|| false)?;
            assert_eq!(original.manifest.ledger_uuid, target.ledger_uuid);
            assert_eq!(original.manifest.counts["transactions"], "0");
            Ok(())
        })
        .unwrap();
}

#[test]
fn budgets_use_economic_share_and_goals_use_deadline_boundaries() {
    use financial_gps::{
        domain::Clock,
        ledger_engine::catalog,
        planning::{budgets, goals, shared},
    };
    struct Fixed;
    impl Clock for Fixed {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            "2026-01-31T12:00:00Z".parse().unwrap()
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager.create("main", draft("plan", false)).unwrap().scope;
    manager.write_domain("main",&scope,|tx|{
        let ctx=Context::new(financial_gps::storage::SqliteSession(tx),&scope,&Fixed)?;
        let account=account(&ctx)?;
        let category=catalog::category_save(&ctx,None,&dto(json!({"name":"Food","kind":"expense","hidden":false,"ignored":false,"archived":false})))?;
        let mut posting_draft=tx_draft(&account);
        posting_draft.category_id=Some(category.id.clone());
        let posting=transactions::save(&ctx,None,&posting_draft)?;
        let group=shared::save(&ctx,None,&dto(json!({"name":"Household","kind":"social"})))?;
        let group=shared::member_save(&ctx,&group.group.id,None,&dto(json!({"label":"Friend"})))?;
        let self_id=&group.members[0].id;
        let friend=&group.members[1].id;
        let expense=dto(json!({"groupId":group.group.id,"payerId":self_id,"transactionId":posting.row.id,
            "date":"2026-01-15","description":"Lunch","amount":money("USD",1000),"method":"equal","shares":[{"memberId":self_id},{"memberId":friend}]}));
        shared::expense_save(&ctx,None,&expense)?;
        let entry=dto(json!({"key":{"categoryId":category.id},"amount":money("USD",700)}));
        let actual=budgets::save(&ctx,&[entry],false)?;
        assert_eq!(actual.rows[0].actual.minor,"500");
        assert_eq!(actual.rows[0].remaining.minor,"200");
        assert!(actual.rows[0].carryover_eligible);
        let goal=goals::save(&ctx,None,&dto(json!({"name":"Rainy day","target":money("USD",1000),
            "startsOn":"2026-01-01","targetOn":"2026-01-31","state":"active",
            "source":{"kind":"manual","current":money("USD",600)}})))?;
        assert!(matches!(goal.state,goals::State::Active));
        assert_eq!(goal.monthly_contribution.minor,"0");
        assert!(matches!(goals::get(&ctx,&goal.id,Some("2026-02-01"))?.state,goals::State::Overdue));
        Ok(())
    }).unwrap();
}

#[test]
fn fx_csv_preview_is_read_only_and_undo_reverts_the_rate() {
    use financial_gps::ledger_engine::fx;
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("fx CSV", false))
        .unwrap()
        .scope;
    let imports = Stage::default();
    let undo = Stage::default();
    let content = b"base,quote,date,rate\nUSD,EUR,2026-01-15,0.92\nBAD,USD,2026-01-15,1\n";
    let first = manager
        .read_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            let result =
                importers::preview_structured(&ctx, &imports, "fx.csv", content, "fx", &|| false)?;
            assert_eq!(fx::list(&ctx, None, None, &page())?.rows.len(), 0);
            Ok(result)
        })
        .unwrap();
    assert_eq!((first.ready, first.errors), (1, 1));
    let batch = manager
        .write_domain("main", &scope, |tx| {
            importers::commit(
                &Context::system(financial_gps::storage::SqliteSession(tx), &scope)?,
                &imports,
                &first.preview_token,
                &[],
                &|| false,
            )
        })
        .unwrap();
    let preview = manager
        .read_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            let rates = fx::list(&ctx, None, None, &page())?;
            assert_eq!(rates.rows[0].draft.rate_decimal, "0.92");
            let repeated = importers::preview_structured(
                &ctx,
                &imports,
                "repeated.csv",
                content,
                "fx",
                &|| false,
            )?;
            assert_eq!(repeated.duplicate, 1);
            importers::undo_preview(&ctx, &undo, &batch.id, None)
        })
        .unwrap();
    assert!(preview.rows[0].blocked.is_none());
    manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            assert!(importers::undo_apply(&ctx, &undo, &preview.preview_token, &[1])?.fully_undone);
            assert!(fx::list(&ctx, None, None, &page())?.rows.is_empty());
            Ok(())
        })
        .unwrap();
}

#[test]
fn protection_transition_rekeys_a_ready_ledger_and_reopens_with_new_credentials() {
    use financial_gps::storage::OpenTarget;
    let root = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(root.path()).unwrap();
    let first = manager.create("main", draft("rekey", false)).unwrap();
    let item = manager
        .write_domain("main", &first.scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &first.scope)?;
            let id = account(&ctx)?;
            transactions::save(&ctx, None, &tx_draft(&id))
        })
        .unwrap();
    let mutation = Mutation {
        scope: first.scope.clone(),
        request_id: new_id(),
        expected_revision: None,
    };
    let result = manager
        .change_protection(
            "main",
            &mutation,
            financial_gps::storage::ProtectionCredentials {
                protection: Protection::Encrypted,
                old: None,
                new: Some(credential("upgrade secret")),
                safety: Some(credential("independent safety archive key")),
            },
            &|| false,
            || Ok(()),
        )
        .unwrap();
    assert_eq!(result.ledger_uuid, first.ledger_uuid);
    assert_eq!(result.protection, Protection::Encrypted);
    assert_ne!(result.scope.session_id, first.scope.session_id);
    manager.close("main", &result.scope).unwrap();
    let opened = manager
        .open(
            "main",
            uuid::Uuid::new_v4(),
            result.scope.ledger_instance_id.clone(),
            || {
                Ok(OpenTarget::Registered(
                    result.scope.ledger_instance_id.parse().unwrap(),
                ))
            },
        )
        .unwrap();
    let challenge = opened.unlock_challenge.unwrap();
    assert!(
        manager
            .unlock("main", &challenge.id, Some(credential("wrong")))
            .is_err()
    );
    let next = manager
        .unlock("main", &challenge.id, Some(credential("upgrade secret")))
        .unwrap();
    manager
        .read_domain("main", &next.scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &next.scope)?;
            assert_eq!(transactions::get(&ctx, &item.row.id)?.row.id, item.row.id);
            Ok(())
        })
        .unwrap();
}

#[test]
fn statement_reconciliation_promotes_and_undo_restores_a_generated_occurrence() {
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("recurring statement", false))
        .unwrap()
        .scope;
    let (account_id,rule,posting)=manager.write_domain("main",&scope,|tx|{
        let ctx=Context::system(financial_gps::storage::SqliteSession(tx),&scope)?;
        let account=account(&ctx)?;
        let rule=recurring::save(&ctx,None,&dto(json!({"accountId":account,"amount":money("USD",1000),
            "direction":"debit","description":"Coffee","notes":"Keep this note","frequency":"monthly","startsOn":"2026-01-15",
            "weekendPolicy":"none","active":true})))?;
        let occurrence=recurring::occurrence_apply(&ctx,&rule.id,"2026-01-15",&recurring::OccurrenceAction::Generate,None)?;
        Ok((account,rule.id,occurrence.transaction_id.unwrap()))
    }).unwrap();
    let imports = Stage::default();
    let undo = Stage::default();
    let preview = manager
        .read_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            let request = importers::PreviewRequest {
                account_id,
                format: Some(StatementFormat::Csv),
                template_id: None,
                parser_options: None,
                detect_duplicates: None,
            };
            importers::preview(
                &ctx,
                &imports,
                "statement.csv",
                b"Date,Description,Amount\n2026-01-15,Coffee,-10.00\n",
                &request,
                &|| false,
            )
        })
        .unwrap();
    assert_eq!(preview.review, 1);
    let ordinal = manager
        .read_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            Ok(
                importers::preview_rows(&ctx, &imports, &preview.preview_token, None, &page())?
                    .rows[0]
                    .ordinal,
            )
        })
        .unwrap();

    let batch = manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            let decision = importers::RowDecision {
                ordinal,
                action: importers::RowAction::Reconcile {
                    transaction_id: posting.clone(),
                },
            };
            let batch =
                importers::commit(&ctx, &imports, &preview.preview_token, &[decision], &|| {
                    false
                })?;
            let entry = transactions::get(&ctx, &posting)?;
            assert_eq!(entry.draft.notes, "Keep this note");
            assert_eq!(entry.row.status.as_str(), "posted");
            let state: String = ctx.tx.query_row(
                "SELECT state FROM recurring_occurrences WHERE rule_id=?1",
                [&rule],
                |r| r.get(0),
            )?;
            assert_eq!(state, "matched");
            Ok(batch)
        })
        .unwrap();
    let undo = manager
        .read_domain("main", &scope, |tx| {
            importers::undo_preview(
                &Context::system(financial_gps::storage::SqliteSession(tx), &scope)?,
                &undo,
                &batch.id,
                None,
            )
            .map(|preview| (preview, &undo))
        })
        .unwrap();
    assert!(undo.0.rows[0].blocked.is_none());
    manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            assert!(
                importers::undo_apply(&ctx, undo.1, &undo.0.preview_token, &[ordinal])?
                    .fully_undone
            );
            let entry = transactions::get(&ctx, &posting)?;
            assert_eq!(entry.row.status.as_str(), "projected");
            assert!(entry.sources.is_empty());
            let state: String = ctx.tx.query_row(
                "SELECT state FROM recurring_occurrences WHERE rule_id=?1",
                [&rule],
                |r| r.get(0),
            )?;
            assert_eq!(state, "generated");
            Ok(())
        })
        .unwrap();
}

#[test]
fn rule_preview_binds_manual_override_policy_and_existing_category_opt_in() {
    use financial_gps::ledger_engine::{VersionedId, catalog, transactions::LedgerFilter};
    use financial_gps::rules::{OverridePolicy, RuleSelection};
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("Rule audit", false))
        .unwrap()
        .scope;
    let (posting_id,first_category,target_category)=manager.write_domain("main",&scope,|tx|{
        let ctx=Context::system(financial_gps::storage::SqliteSession(tx),&scope)?;
        let account=account(&ctx)?;
        let category=|name|catalog::category_save(&ctx,None,&dto(json!({"name":name,"kind":"expense","hidden":false,"ignored":false,"archived":false})));
        let first=category("Original")?.id;
        let target=category("Rule output")?.id;
        let posting=transactions::save(&ctx,None,&tx_draft(&account))?;
        let mut changed=posting.draft;
        changed.category_id=Some(first.clone());
        let posting=transactions::save(&ctx,Some(&VersionedId{id:posting.row.id,version:posting.row.version}),&changed)?;
        // This represents a manual category override on an imported posting.
        tx.execute("INSERT OR IGNORE INTO transaction_overrides VALUES (?1,'category_id')",[&posting.row.id]).map_err(financial_gps::storage::sql_error)?;
        Ok((posting.row.id,first,target))
    }).unwrap();
    let draft = |overwrite| {
        dto(
            json!({"name":"Route expense","priority":1,"enabled":true,"applyExisting":true,
        "overwriteExistingCategory":overwrite,"schemaVersion":1,
        "conditions":{"op":"and","children":[{"kind":"predicate","value":{"field":"description","operator":"contains",
            "value":{"type":"text","value":"strasse"}}}]},
        "actions":[{"kind":"category","value":target_category}]}),
        )
    };
    let stages = Stage::default();
    for (overwrite, allow, expected) in [(false, true, "0"), (true, false, "0"), (true, true, "1")]
    {
        let policy = if allow {
            OverridePolicy::AllowExplicit
        } else {
            OverridePolicy::Preserve
        };
        let rule = manager
            .write_domain("main", &scope, |tx| {
                rules::save(
                    &Context::system(financial_gps::storage::SqliteSession(tx), &scope)?,
                    None,
                    &draft(overwrite),
                )
            })
            .unwrap();
        let preview = manager
            .read_domain("main", &scope, |tx| {
                let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
                let preview = rules::preview(
                    &ctx,
                    &stages,
                    RuleSelection::Saved {
                        ids: vec![rule.id.clone()],
                    },
                    LedgerFilter::default(),
                    10,
                    policy.clone(),
                    &|| false,
                )?;
                assert_eq!(
                    transactions::get(&ctx, &posting_id)?
                        .row
                        .category_id
                        .as_deref(),
                    Some(first_category.as_str())
                );
                Ok(preview)
            })
            .unwrap();
        assert_eq!(preview.would_change, expected);
        manager
            .write_domain("main", &scope, |tx| {
                let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
                let opposite = if allow {
                    OverridePolicy::Preserve
                } else {
                    OverridePolicy::AllowExplicit
                };
                assert_eq!(
                    rules::apply(&ctx, &stages, &preview.preview_token, &opposite, &|| false)
                        .unwrap_err()
                        .message_key,
                    "rule_preview_override_policy_changed"
                );
                assert_eq!(
                    rules::apply(&ctx, &stages, &preview.preview_token, &policy, &|| false)?
                        .changed,
                    expected
                );
                Ok(())
            })
            .unwrap();
    }
    manager
        .read_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            assert_eq!(
                transactions::get(&ctx, &posting_id)?
                    .row
                    .category_id
                    .as_deref(),
                Some(target_category.as_str())
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn loan_contract_limit_and_positive_rate_conserve_all_minor_units() {
    let date = parse_date("2026-01-31").unwrap();
    for amortization in ["price", "sac"] {
        let mut term: loans::LoanTerms = dto(
            json!({"accountId":"loan","principal":money("USD",12_000_013),
            "interestRatePercent":"2.4","ratePeriod":"yearlyEffective","amortization":amortization,
            "kind":"personal","termMonths":1200,"firstPaymentDate":"2026-01-31"}),
        );
        let rows = loans::calculate(&term, date).unwrap().rows;
        assert_eq!(rows.len(), 1200);
        assert!(rows[0].interest.minor.parse::<u64>().unwrap() > 0);
        assert_eq!(rows.last().unwrap().balance.minor, "0");
        assert_eq!(
            rows.iter()
                .map(|row| row.principal.minor.parse::<u64>().unwrap())
                .sum::<u64>(),
            12_000_013
        );
        term.term_months = 1201;
        assert!(loans::calculate(&term, date).is_err());
    }
}

#[test]
fn idr_template_export_import_is_equivalent_in_json_and_toml() {
    use financial_gps::importers::{DocumentFormat, TemplateDraft};
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("Templates", false))
        .unwrap()
        .scope;
    let template: TemplateDraft = dto(
        json!({"name":"Bank IDR","institution":"Fixture","format":"csv","schemaVersion":1,
        "options":{"currency":"IDR","dateStyle":"dayMonthYear","csv":{"dateColumn":0,"descriptionColumn":1,
           "inflowColumn":2,"outflowColumn":3,"delimiter":";","decimalSeparator":",","groupSeparator":".","invertSign":true}}}),
    );
    manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            let saved = importers::template_save(&ctx, None, &template)?;
            let original = serde_json::to_value(&template).unwrap();
            for format in [DocumentFormat::Json, DocumentFormat::Toml] {
                let bytes = importers::encode_template(&saved, &format)?;
                let decoded = importers::decode_template(&bytes, &format)?;
                assert_eq!(serde_json::to_value(&decoded).unwrap(), original);
                let rows = parse(
                    StatementFormat::Csv,
                    b"Date;Description;In;Out\n15/01/2026;Fee;;1.234.567\n",
                    &decoded.options,
                    "IDR",
                    0,
                )?;
                assert_eq!(ready(&rows).amount_minor, "1234567");
                assert_eq!(ready(&rows).direction.as_str(), "credit");
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn budget_copy_rolls_back_on_collision_and_requires_positive_rounded_limits() {
    use financial_gps::{ledger_engine::catalog, planning::budgets};
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("Budget copy", false))
        .unwrap()
        .scope;
    let feb = "2026-02";
    let jan = "2026-01";
    let keys=manager.write_domain("main",&scope,|tx|{
        let ctx=Context::system(financial_gps::storage::SqliteSession(tx),&scope)?;
        let mut categories=Vec::new();
        for name in ["A","Z"] {
            categories.push(catalog::category_save(&ctx,None,&dto(json!({"name":name,"kind":"expense"})))?.id);
        }
        let entry=|category:&str,month:&str,minor|dto(json!({"key":{"categoryId":category,"month":month},"amount":money("USD",minor)}));
        budgets::save(&ctx,&[entry(&categories[0],jan,125),entry(&categories[1],jan,250)],false)?;
        budgets::save(&ctx,&[entry(&categories[1],feb,100)],false)?;
        Ok(categories)
    }).unwrap();
    assert!(
        manager
            .write_domain("main", &scope, |tx| budgets::copy(
                &Context::system(financial_gps::storage::SqliteSession(tx), &scope)?,
                jan,
                feb,
                "10",
                false
            ))
            .is_err()
    );
    manager
        .read_domain("main", &scope, |tx| {
            let rows = budgets::get(
                &Context::system(financial_gps::storage::SqliteSession(tx), &scope)?,
                feb,
            )?
            .rows;
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].category_id, keys[1]);
            assert_eq!(rows[0].limit.minor, "100");
            Ok(())
        })
        .unwrap();
    assert!(
        manager
            .write_domain("main", &scope, |tx| budgets::copy(
                &Context::system(financial_gps::storage::SqliteSession(tx), &scope)?,
                jan,
                feb,
                "-100",
                true
            ))
            .is_err()
    );
    let result = manager
        .write_domain("main", &scope, |tx| {
            budgets::copy(
                &Context::system(financial_gps::storage::SqliteSession(tx), &scope)?,
                jan,
                feb,
                "10",
                true,
            )
        })
        .unwrap();
    assert_eq!(
        result
            .rows
            .iter()
            .map(|row| row.limit.minor.as_str())
            .collect::<Vec<_>>(),
        ["138", "275"]
    );
}

#[test]
fn goal_pace_classifies_exactly_five_and_ten_percentage_point_boundaries() {
    use financial_gps::{domain::Clock, ledger_engine::VersionedId, planning::goals};
    struct Halfway;
    impl Clock for Halfway {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            "2026-02-20T12:00:00Z".parse().unwrap()
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("Goal pace", false))
        .unwrap()
        .scope;
    manager.write_domain("main",&scope,|tx|{
        let ctx=Context::new(financial_gps::storage::SqliteSession(tx),&scope,&Halfway)?;
        let mut goal:goals::GoalDraft=dto(json!({"name":"Goal","target":money("USD",1000),
            "startsOn":"2026-01-01","targetOn":"2026-04-11","state":"active","source":{"kind":"manual","current":money("USD",0)}}));
        let saved=goals::save(&ctx,None,&goal)?;
        let saved=goals::get(&ctx,&saved.id,None)?;
        let target=VersionedId{id:saved.id,version:saved.version};
        for (minor,pace) in [(449,"behind"),(450,"onTrack"),(599,"onTrack"),(600,"ahead")] {
            goal.source=goals::Source::Manual{current:money("USD",minor)};
            let pace_result=goals::save(&ctx,Some(&target),&goal)?.pace;
            assert_eq!(serde_json::to_value(pace_result).unwrap(),json!(pace));
        }
        Ok(())
    }).unwrap();
}

#[test]
fn attachment_count_byte_limit_and_signature_are_enforced() {
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("Attachment caps", false))
        .unwrap()
        .scope;
    let posting = manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            let account = account(&ctx)?;
            Ok(transactions::save(&ctx, None, &tx_draft(&account))?.row.id)
        })
        .unwrap();
    let grants = PathGrants::default();
    let submit = |paths: &[std::path::PathBuf]| {
        let mutation = Mutation {
            scope: scope.clone(),
            request_id: new_id(),
            expected_revision: None,
        };
        manager.attachment_add(
            "main",
            &mutation,
            &posting,
            &paths.len(),
            || {
                paths
                    .iter()
                    .map(|path| {
                        let grant = grants.issue("main", PathPurpose::AttachmentRead, path)?;
                        grants.consume("main", PathPurpose::AttachmentRead, &grant.id)
                    })
                    .collect()
            },
            || Ok(()),
        )
    };
    let too_large = temp.path().join("oversize.pdf");
    std::fs::write(&too_large, b"%PDF-1.7").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&too_large)
        .unwrap()
        .set_len(10_000_001)
        .unwrap();
    assert!(submit(&[too_large]).is_err());
    let bad_signature = temp.path().join("not-a-pdf.pdf");
    std::fs::write(&bad_signature, b"GIF89a").unwrap();
    assert!(submit(&[bad_signature]).is_err());
    let files = (0..11)
        .map(|index| {
            let path = temp.path().join(format!("receipt-{index}.pdf"));
            std::fs::write(&path, b"%PDF-1.7").unwrap();
            path
        })
        .collect::<Vec<_>>();
    assert!(submit(&files).is_err());
    assert_eq!(submit(&files[..10]).unwrap().value.attachments.len(), 10);
    assert!(submit(&files[10..]).is_err());
}

struct At(chrono::DateTime<chrono::Utc>);
impl financial_gps::domain::Clock for At {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.0
    }
}

#[test]
fn scheduled_backup_catches_up_after_unlock_and_retains_ten_in_selected_directory() {
    use financial_gps::{
        crypto::{BackupSecretStore, OsKeychain},
        storage::schedule,
    };
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    #[derive(Default)]
    struct Secrets(Mutex<HashMap<uuid::Uuid, std::sync::Arc<Credential>>>);
    impl BackupSecretStore for Secrets {
        fn remember(&self, id: uuid::Uuid, _: &Credential) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(id, Arc::new(credential("fixture schedule passphrase")));
            Ok(())
        }
        fn retrieve(&self, id: uuid::Uuid) -> Result<Credential> {
            assert!(self.0.lock().unwrap().contains_key(&id));
            Ok(credential("fixture schedule passphrase"))
        }
        fn revoke(&self, id: uuid::Uuid) -> Result<()> {
            self.0.lock().unwrap().remove(&id);
            Ok(())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let selected = tempfile::tempdir().unwrap();
    let manager = LedgerManager::with_secret_stores(
        root.path(),
        Arc::new(OsKeychain),
        Arc::new(Secrets::default()),
    )
    .unwrap();
    let mut input = draft("Scheduled", true);
    input.timezone = Some("UTC".into());
    let session = manager.create("main", input).unwrap();
    let scope = session.scope;
    let grants = PathGrants::default();
    let grant = grants
        .issue("main", PathPurpose::BackupDirectory, selected.path())
        .unwrap();
    let mutation = Mutation {
        scope: scope.clone(),
        request_id: new_id(),
        expected_revision: None,
    };
    let schedule = schedule::ScheduleChange {
        schedule: Some(schedule::Schedule {
            frequency: schedule::BackupFrequency::Daily,
            local_time: "03:00".into(),
            weekday: None,
            retain_days: None,
        }),
        remember_secret: Some(true),
    };
    let status = manager
        .set_backup_schedule(
            "main",
            &mutation,
            &schedule,
            Some(&grant.id),
            || {
                grants
                    .consume("main", PathPurpose::BackupDirectory, &grant.id)
                    .map(Some)
            },
            Some(&credential("fixture schedule passphrase")),
        )
        .unwrap();
    assert_eq!(
        status.value.destination_name.as_deref(),
        selected.path().file_name().and_then(|name| name.to_str())
    );
    manager.lock("main", &scope).unwrap();
    let when: chrono::DateTime<chrono::Utc> = "2026-11-22T04:00:00Z".parse().unwrap();
    manager.run_scheduled_backup_at("main", &At(when)).unwrap();
    assert_eq!(selected.path().read_dir().unwrap().count(), 0);
    let open = manager
        .open(
            "main",
            uuid::Uuid::new_v4(),
            scope.ledger_instance_id.clone(),
            || {
                Ok(financial_gps::storage::OpenTarget::Registered(
                    scope.ledger_instance_id.parse().unwrap(),
                ))
            },
        )
        .unwrap();
    let decrypt = draft("Scheduled", true).credential;
    let after = manager
        .unlock("main", &open.unlock_challenge.unwrap().id, decrypt)
        .unwrap();
    let backups = selected
        .path()
        .join(format!("financial-gps-{}", scope.ledger_instance_id));
    manager.run_scheduled_backup_at("main", &At(when)).unwrap();
    manager.run_scheduled_backup_at("main", &At(when)).unwrap();
    let paths = || {
        backups
            .read_dir()
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>()
    };
    assert_eq!(paths().len(), 1);
    let first = paths().pop().unwrap();
    let archive = snapshot::open(
        std::fs::File::open(&first).unwrap(),
        &credential("fixture schedule passphrase"),
        &|| false,
    )
    .unwrap();
    assert_eq!(archive.manifest.ledger_uuid, session.ledger_uuid);
    for day in 1..=10 {
        manager
            .run_scheduled_backup_at("main", &At(when + chrono::Duration::days(day)))
            .unwrap();
    }
    assert_eq!(paths().len(), 10);
    assert!(!first.exists());
    assert!(!root.path().join("backups").exists());
    manager
        .read_domain("main", &after.scope, |tx| {
            let status = schedule::get(&Context::new(
                financial_gps::storage::SqliteSession(tx),
                &after.scope,
                &At(when),
            )?)?;
            assert!(status.last_success.is_some());
            Ok(())
        })
        .unwrap();
}

#[test]
fn ledger_midnight_and_changed_system_zone_are_resolved_per_context() {
    use chrono::{DateTime, NaiveDate, Utc};
    use financial_gps::domain::Clock;
    struct Local {
        now: DateTime<Utc>,
        zone: chrono_tz::Tz,
    }
    impl Clock for Local {
        fn now(&self) -> DateTime<Utc> {
            self.now
        }
        fn local_date(&self, instant: DateTime<Utc>) -> NaiveDate {
            instant.with_timezone(&self.zone).date_naive()
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let mut input = draft("Clock", false);
    input.timezone = Some("Asia/Jakarta".into());
    let scope = manager.create("main", input).unwrap().scope;
    let date = |value: &str, zone| {
        manager
            .read_domain("main", &scope, |tx| {
                Ok(Context::new(
                    financial_gps::storage::SqliteSession(tx),
                    &scope,
                    &Local {
                        now: value.parse().unwrap(),
                        zone,
                    },
                )?
                .today
                .to_string())
            })
            .unwrap()
    };
    assert_eq!(date("2026-11-22T16:59:59Z", chrono_tz::UTC), "2026-11-22");
    assert_eq!(date("2026-11-22T17:00:00Z", chrono_tz::UTC), "2026-11-23");
    manager
        .write("main", &scope, |tx| {
            tx.execute("UPDATE ledger_meta SET timezone_override=NULL", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(date("2026-11-22T17:00:00Z", chrono_tz::UTC), "2026-11-22");
    assert_eq!(
        date("2026-11-22T17:00:00Z", chrono_tz::Asia::Jakarta),
        "2026-11-23"
    );
}

#[test]
fn csv_identity_review_and_card_reconciliation_preserve_manual_fields_and_exposure() {
    use financial_gps::ledger_engine::{VersionedId, catalog};
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("Card import", false))
        .unwrap()
        .scope;
    let clock = At("2026-11-22T12:00:00Z".parse().unwrap());
    let (card,id,cat,payee)=manager.write_domain("main",&scope,|tx|{
        let ctx=&Context::new(financial_gps::storage::SqliteSession(tx),&scope,&clock)?;
        let card=accounts::save(ctx,None,&dto(json!({"name":"Card","kind":"creditCard","currencyCode":"USD","closeDay":24,"dueDay":18,"cardLimitMinor":"10000"})))?;
        let category=catalog::category_save(ctx,None,&dto(json!({"name":"Coffee","kind":"expense"})))?;
        let payee=catalog::payee_save(ctx,None,&dto(json!({"name":"Coffee shop"})))?;
        let mut posting=tx_draft(&card.id);
        posting.bank_date="2026-11-20".into();
        posting.status=financial_gps::ledger_engine::TxStatus::Pending;
        posting.description="Cafe".into();
        let posting=transactions::save(ctx,None,&posting)?;
        Ok((card.id,posting.row.id,category.id,payee.id))
    }).unwrap();
    let stages = Stage::default();
    let request = |dedup| imports_request(&card, dedup);
    let input = b"Date,Description,Amount\n2026-11-21,Cafe,-10.00\n";
    let preview = |bytes: &[u8], dedup| {
        manager
            .read_domain("main", &scope, |tx| {
                let ctx = Context::new(financial_gps::storage::SqliteSession(tx), &scope, &clock)?;
                importers::preview(&ctx, &stages, "card.csv", bytes, &request(dedup), &|| false)
            })
            .unwrap()
    };
    let pending = preview(input, None);
    assert_eq!(pending.review, 1);
    assert_eq!(preview(input, Some(false)).ready, 1);
    let twice =
        b"Date,Description,Amount\n2026-11-01,Stra\xc3\x9fe,-3.00\n2026-11-01,STRASSE,-3.00\n";
    assert_eq!(preview(twice, None).review, 2);
    assert_eq!(preview(twice, Some(false)).ready, 2);
    let commit = |summary: &importers::ImportPreview| {
        manager
            .write_domain("main", &scope, |tx| {
                let ctx = Context::new(financial_gps::storage::SqliteSession(tx), &scope, &clock)?;
                let rows =
                    importers::preview_rows(&ctx, &stages, &summary.preview_token, None, &page())?;
                let choices = [importers::RowDecision {
                    ordinal: rows.rows[0].ordinal,
                    action: importers::RowAction::Reconcile {
                        transaction_id: id.clone(),
                    },
                }];
                importers::commit(&ctx, &stages, &summary.preview_token, &choices, &|| false)
            })
            .unwrap()
    };
    let balance = || {
        manager
            .read_domain("main", &scope, |tx| {
                let ctx = Context::new(financial_gps::storage::SqliteSession(tx), &scope, &clock)?;
                let card = accounts::get(&ctx, &card, None)?;
                let credit = card.credit.unwrap();
                Ok((
                    card.balance.minor,
                    credit.debt.minor,
                    credit.available.unwrap().minor,
                ))
            })
            .unwrap()
    };
    let exposure = balance();
    assert_eq!(exposure, ("-1000".into(), "1000".into(), "9000".into()));
    commit(&pending);
    assert_eq!(balance(), exposure);
    manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::new(financial_gps::storage::SqliteSession(tx), &scope, &clock)?;
            let current = transactions::get(&ctx, &id)?;
            let version = VersionedId {
                id: id.clone(),
                version: current.row.version,
            };
            let mut changes = [
                current.draft.clone(),
                current.draft.clone(),
                current.draft.clone(),
                current.draft.clone(),
            ];
            changes[0].bank_date = "2026-11-19".into();
            changes[1].amount.minor = "1050".into();
            changes[2].amount.currency = "EUR".into();
            changes[3].direction = financial_gps::ledger_engine::Direction::Credit;
            for draft in changes {
                assert!(transactions::save(&ctx, Some(&version), &draft).is_err());
            }
            let mut edited = current.draft;
            edited.description = "Manual description".into();
            edited.notes = "Manual note #local".into();
            edited.payee_id = Some(payee.clone());
            edited.category_id = Some(cat.clone());
            edited.reporting_date_override = Some("2026-11-22".into());
            transactions::save(&ctx, Some(&version), &edited)?;
            Ok(())
        })
        .unwrap();
    let again = preview(input, None);
    assert_eq!(again.review, 1);
    commit(&again);
    assert_eq!(balance(), exposure);
    manager
        .read_domain("main", &scope, |tx| {
            let ctx = Context::new(financial_gps::storage::SqliteSession(tx), &scope, &clock)?;
            let item = transactions::get(&ctx, &id)?;
            assert_eq!(item.row.status.as_str(), "posted");
            assert_eq!(item.draft.description, "Manual description");
            assert_eq!(item.draft.notes, "Manual note #local");
            assert_eq!(item.draft.payee_id.as_deref(), Some(payee.as_str()));
            assert_eq!(item.draft.category_id.as_deref(), Some(cat.as_str()));
            assert_eq!(item.row.reporting_date, "2026-11-22");
            assert_eq!(
                transactions::list(&ctx, &dto(json!({"accountIds":[card]})), &page(), false)?
                    .rows
                    .len(),
                1
            );
            Ok(())
        })
        .unwrap();
}
fn imports_request(account: &str, detect_duplicates: Option<bool>) -> importers::PreviewRequest {
    importers::PreviewRequest {
        account_id: account.into(),
        format: Some(StatementFormat::Csv),
        template_id: None,
        parser_options: None,
        detect_duplicates,
    }
}

#[test]
fn two_imported_postings_link_as_one_transfer_without_recreating_either_leg() {
    use financial_gps::ledger_engine::{VersionedId, transfers};
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("Transfer import", false))
        .unwrap()
        .scope;
    let (debit, credit) = manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            Ok((account(&ctx)?, account(&ctx)?))
        })
        .unwrap();
    let stages = Stage::default();
    let import =
        |account: &str, bytes: &[u8]| {
            let preview = manager
                .read_domain("main", &scope, |tx| {
                    importers::preview(
                        &Context::system(financial_gps::storage::SqliteSession(tx), &scope)?,
                        &stages,
                        "transfer.csv",
                        bytes,
                        &imports_request(account, None),
                        &|| false,
                    )
                })
                .unwrap();
            manager
                .write_domain("main", &scope, |tx| {
                    let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
                    importers::commit(&ctx, &stages, &preview.preview_token, &[], &|| false)?;
                    Ok(transactions::list(
                        &ctx,
                        &dto(json!({"accountIds":[account]})),
                        &page(),
                        false,
                    )?
                    .rows
                    .remove(0))
                })
                .unwrap()
        };
    let source = import(
        &debit,
        b"Date,Description,Amount\n2026-11-01,Internal transfer,-10.00\n",
    );
    let destination = import(
        &credit,
        b"Date,Description,Amount\n2026-11-01,Internal transfer,10.00\n",
    );
    manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            let choices = transfers::candidates(&ctx, &source.id, &dto(json!({})), &page())?;
            assert_eq!(choices.rows[0].posting.id, destination.id);
            let existing = |id, version| transfers::ExistingOrDraft::Existing {
                target: VersionedId { id, version },
            };
            let pair = transfers::save(
                &ctx,
                None,
                &existing(source.id.clone(), source.version),
                &existing(destination.id.clone(), destination.version),
                None,
            )?;
            assert_eq!(pair.pair.source.row.id, source.id);
            assert_eq!(pair.pair.destination.row.id, destination.id);
            assert!(
                !pair.pair.source.sources.is_empty() && !pair.pair.destination.sources.is_empty()
            );
            assert!(
                transactions::list(&ctx, &dto(json!({"pnlOnly":true})), &page(), false)?
                    .rows
                    .is_empty()
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn six_recurring_frequencies_skip_first_count_eligible_and_preserve_edit_floor() {
    use financial_gps::ledger_engine::VersionedId;
    let mut base: recurring::RecurringDraft =
        dto(json!({"accountId":"fixture","amount":money("USD",100),
        "direction":"debit","description":"Recurring","frequency":"monthly","startsOn":"2024-01-31",
        "weekendPolicy":"none","skipFirst":true,"countLimit":2,"active":true}));
    for (frequency, dates) in [
        ("weekly", ["2024-02-07", "2024-02-14"]),
        ("biweekly", ["2024-02-14", "2024-02-28"]),
        ("monthly", ["2024-02-29", "2024-03-31"]),
        ("quarterly", ["2024-04-30", "2024-07-31"]),
        ("semiannual", ["2024-07-31", "2025-01-31"]),
        ("yearly", ["2025-01-31", "2026-01-31"]),
    ] {
        base.frequency = dto(json!(frequency));
        let actual = recurring::dates(
            &base,
            parse_date("2024-01-01").unwrap(),
            parse_date("2027-12-31").unwrap(),
        )
        .unwrap();
        assert_eq!(
            actual
                .into_iter()
                .map(|v| v.0.to_string())
                .collect::<Vec<_>>(),
            dates,
            "{frequency}"
        );
    }
    let root = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(root.path()).unwrap();
    let scope = manager
        .create("main", draft("Recurrence floor", false))
        .unwrap()
        .scope;
    manager
        .write_domain("main", &scope, |tx| {
            let clock = At("2024-06-01T12:00:00Z".parse().unwrap());
            let ctx = Context::new(financial_gps::storage::SqliteSession(tx), &scope, &clock)?;
            base.account_id = account(&ctx)?;
            base.frequency = recurring::Frequency::Monthly;
            base.skip_first = false;
            base.count_limit = None;
            let saved = recurring::save(&ctx, None, &base)?;
            let before = recurring::occurrence_apply(
                &ctx,
                &saved.id,
                "2024-02-29",
                &recurring::OccurrenceAction::Generate,
                None,
            )?;
            base.anchor_day = Some(5);
            let changed = recurring::save(
                &ctx,
                Some(&VersionedId {
                    id: saved.id.clone(),
                    version: saved.version,
                }),
                &base,
            )?;
            assert_eq!(changed.generation_floor.as_deref(), Some("2024-06-01"));
            let rows = recurring::for_range(
                &ctx,
                Some(&saved.id),
                parse_date("2024-02-01")?,
                parse_date("2024-06-30")?,
            )?;
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].transaction_id, before.transaction_id);
            assert_eq!(rows[1].nominal_date, "2024-06-05");
            Ok(())
        })
        .unwrap();
}

#[test]
fn rule_pack_collision_policies_and_priority_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager.create("main", draft("Rules", false)).unwrap().scope;
    manager.write_domain("main",&scope,|tx|{
        let ctx=Context::system(financial_gps::storage::SqliteSession(tx),&scope)?;
        let rule=|name,priority|dto(json!({"name":name,"priority":priority,"enabled":true,"applyExisting":true,
        "overwriteExistingCategory":false,"schemaVersion":1,"conditions":{"op":"and","children":[{"kind":"predicate",
        "value":{"field":"description","operator":"contains","value":{"type":"text","value":"coffee"}}}]},
        "actions":[{"kind":"appendNotes","value":"Coffee"}]}));
        let one=rules::save(&ctx,None,&rule("First",0))?;
        let two=rules::save(&ctx,None,&rule("Second",1))?;
        assert_eq!(rules::reorder(&ctx,&[two.id.clone(),one.id.clone()])?[0].id,two.id);
        let pack=rules::export_pack(&ctx,None)?;
        let wire=serde_json::to_vec(&pack).unwrap();
        let mut decoded:rules::RulePack=serde_json::from_slice(&wire).unwrap();
        assert_eq!(decoded.rules[0].name,"Second");
        assert!(rules::import_pack(&ctx,decoded.clone(),&rules::CollisionPolicy::Reject).is_err());
        assert!(rules::import_pack(&ctx,decoded.clone(),&rules::CollisionPolicy::Skip)?.ids.is_empty());
        decoded.rules[0].enabled=false;
        assert_eq!(rules::import_pack(&ctx,decoded,&rules::CollisionPolicy::Replace)?.ids,vec![two.id.clone(),one.id]);
        assert!(!rules::get(&ctx,&two.id)?.draft.enabled);
        Ok(())
    }).unwrap();
}

#[test]
fn goal_sources_use_ledger_values_and_asset_growth_resets_on_new_valuation() {
    use financial_gps::planning::goals;
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("Goal sources", false))
        .unwrap()
        .scope;
    manager.write_domain("main",&scope,|tx|{
        let clock=At("2026-07-01T12:00:00Z".parse().unwrap());
        let ctx=Context::new(financial_gps::storage::SqliteSession(tx),&scope,&clock)?;
        let account=account(&ctx)?;
        let mut deposit=tx_draft(&account);
        deposit.direction=financial_gps::ledger_engine::Direction::Credit;
        deposit.amount=money("USD",400);
        transactions::save(&ctx,None,&deposit)?;
        let asset=assets::save(&ctx,None,&dto(json!({"name":"Investment","kind":"investment","currency":"USD"})))?;
        assets::valuation_save(&ctx,None,&dto(json!({"assetId":asset.id,"valuedOn":"2026-06-01","value":money("USD",600)})))?;
        let group=assets::group_save(&ctx,None,&dto(json!({"name":"Holdings"})),std::slice::from_ref(&asset.id))?;
        for (source,expected) in [
            (json!({"kind":"manual","current":money("USD",500)}),"500"),
            (json!({"kind":"account","accountId":account}),"400"),
            (json!({"kind":"asset","assetId":asset.id}),"600"),
            (json!({"kind":"assetGroup","groupId":group.id}),"600"),
            (json!({"kind":"netWorth"}),"1000")
        ] {
            let goal=goals::save(&ctx,None,&dto(json!({"name":"Track","target":money("USD",2000),"startsOn":"2026-01-01",
                "targetOn":"2027-01-01","source":source,"state":"active"})))?;
            assert_eq!(goal.current.minor,expected);
        }
        for (target,current,percent) in [(0,0,"0"),(0,1,"100"),(-1,-1,"0"),(-1,1,"100")] {
            let goal=goals::save(&ctx,None,&dto(json!({"name":"Boundary","target":money("USD",target),"startsOn":"2026-01-01",
            "targetOn":"2027-01-01","source":{"kind":"manual","current":money("USD",current)},"state":"active"})))?;
            assert_eq!(goal.progress_percent,percent);
        }
        for (interval,end) in [("daily","2024-02-02"),("weekly","2024-02-14"),("monthly","2024-03-31"),("yearly","2026-01-31")] {
            let growth=dto(json!({"startsOn":"2024-01-31","interval":interval,"change":{"kind":"percentage","value":"10"}}));
            let value=assets::project_value(parse_date("2024-01-31")?,parse_date(end)?,&money("USD",100),&[growth])?;
            assert_eq!(value.minor,"121","{interval}");
        }
        let observed=assets::save(&ctx,None,&dto(json!({"name":"Observed","kind":"other","currency":"USD","growth":[
            {"startsOn":"2026-06-01","interval":"daily","change":{"kind":"fixed","value":money("USD",10)}}]})))?;
        for (date,value) in [("2026-06-01",100),("2026-06-10",500)] {
            assets::valuation_save(&ctx,None,&dto(json!({"assetId":observed.id,"valuedOn":date,"value":money("USD",value)})))?;
        }
        assert_eq!(assets::get(&ctx,&observed.id,Some("2026-06-09"))?.value.minor,"180");
        assert_eq!(assets::get(&ctx,&observed.id,Some("2026-06-11"))?.value.minor,"510");
        Ok(())
    }).unwrap();
}

#[test]
fn budget_actual_matches_fx_pnl_and_projected_placeholders_count_once() {
    use financial_gps::{
        ledger_engine::{DateRange, catalog, fx, reports},
        planning::budgets,
    };
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("FX budgets", false))
        .unwrap()
        .scope;
    manager
        .write_domain("main", &scope, |tx| {
            let clock = At("2026-05-10T12:00:00Z".parse().unwrap());
            let ctx = Context::new(financial_gps::storage::SqliteSession(tx), &scope, &clock)?;
            let eur = accounts::save(
                &ctx,
                None,
                &dto(json!({"name":"EUR Account","kind":"checking","currencyCode":"EUR"})),
            )?;
            let category =
                catalog::category_save(&ctx, None, &dto(json!({"name":"Food","kind":"expense"})))?;
            fx::save(
                &ctx,
                None,
                &dto(
                    json!({"pair":{"baseCode":"EUR","quoteCode":"USD"},"rateDate":"2026-05-01",
            "rateDecimal":"2","sourceKind":"manual","sourceKey":"fixture"}),
                ),
            )?;
            let mut posting = tx_draft(&eur.id);
            posting.amount = money("EUR", 100);
            posting.bank_date = "2026-05-05".into();
            posting.category_id = Some(category.id.clone());
            transactions::save(&ctx, None, &posting)?;
            let rule = recurring::save(
                &ctx,
                None,
                &dto(json!({"accountId":eur.id,"amount":money("EUR",50),
            "direction":"debit","description":"Meals","frequency":"weekly","startsOn":"2026-05-12",
            "weekendPolicy":"none","countLimit":2,"categoryId":category.id,"active":true})),
            )?;
            recurring::occurrence_apply(
                &ctx,
                &rule.id,
                "2026-05-12",
                &recurring::OccurrenceAction::Generate,
                None,
            )?;
            let entry = dto(json!({"key":{"categoryId":category.id},"amount":money("USD",1000)}));
            let value = budgets::save(&ctx, &[entry], false)?;
            let budget = &value.rows[0];
            let filter = transactions::LedgerFilter {
                category_ids: vec![category.id],
                range: Some(DateRange {
                    from: Some("2026-05-01".into()),
                    to: Some("2026-05-31".into()),
                }),
                ..Default::default()
            };
            let report = reports::report(
                &ctx,
                &reports::ReportKind::CategoryTrends,
                &filter,
                &reports::Granularity::Month,
                Some("USD"),
            )?;
            assert_eq!(report.points[0].categories[0].amount.reliable.minor, "200");
            assert_eq!(
                budget.actual.minor,
                report.points[0].categories[0].amount.reliable.minor
            );
            assert_eq!(budget.projected.minor, "400");
            assert_eq!(budget.remaining.minor, "600");
            Ok(())
        })
        .unwrap();
}

#[test]
fn exact_and_percentage_shared_expenses_reconcile_cross_currency_only_with_provenance() {
    use financial_gps::{ledger_engine::Direction, planning::shared};
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager
        .create("main", draft("Shared FX", false))
        .unwrap()
        .scope;
    manager.write_domain("main",&scope,|tx|{
        let ctx=Context::new(financial_gps::storage::SqliteSession(tx),&scope,&At("2026-07-01T12:00:00Z".parse().unwrap()))?;
        let group=shared::save(&ctx,None,&dto(json!({"name":"Trip","kind":"trip"})))?;
        let group=shared::member_save(&ctx,&group.group.id,None,&dto(json!({"label":"Friend"})))?;
        let own=&group.members[0].id;
        let friend=&group.members[1].id;
        let expense=|currency: &str,minor,method,first,second|dto(json!({"groupId":group.group.id,"payerId":own,
            "date":"2026-01-15","description":"Travel","amount":{"currency":currency,"minor":minor},
            "method":method,"shares":[{"memberId":own,"value":first},{"memberId":friend,"value":second}]}));
        shared::expense_save(&ctx,None,&expense("EUR","100","exact","40","60"))?;
        let balances=shared::expense_save(&ctx,None,&expense("USD","101","percentage","33","67"))?;
        assert_eq!(balances.balances.iter().find(|v|v.member_id==*own&&v.amount.currency=="EUR").unwrap().amount.minor,"60");
        assert_eq!(balances.balances.iter().find(|v|v.member_id==*own&&v.amount.currency=="USD").unwrap().amount.minor,"68");
        let account=account(&ctx)?;
        let mut posting=tx_draft(&account);
        posting.direction=Direction::Credit;
        posting.bank_date="2026-02-01".into();
        posting.amount=money("USD",120);
        let unproven=transactions::save(&ctx,None,&posting)?;
        let settlement=|amount,link|dto(json!({"groupId":group.group.id,"fromMemberId":friend,
            "toMemberId":own,"date":"2026-02-01","amount":money("EUR",amount),"transactionId":link}));
        assert_eq!(shared::settlement_save(&ctx,None,&settlement(61,None::<&str>)).unwrap_err().message_key,"settlement_exceeds_outstanding_balance");
        assert_eq!(shared::settlement_save(&ctx,None,&settlement(60,Some(unproven.row.id.as_str()))).unwrap_err().message_key,
            "settlement_fx_provenance_required");
        posting.original=Some(dto(json!({"amount":money("EUR",60),"rateDecimal":"2","rateDate":"2026-02-01","rateSource":"manual"})));
        let proven=transactions::save(&ctx,None,&posting)?;
        let result=shared::settlement_save(&ctx,None,&settlement(60,Some(proven.row.id.as_str())))?;
        assert!(result.balances.iter().all(|b|b.amount.currency=="USD"));
        let pnl=transactions::list(&ctx,&dto(json!({"pnlOnly":true})),&page(),false)?;
        assert!(!pnl.rows.iter().any(|row|row.id==proven.row.id));
        Ok(())
    }).unwrap();
}

#[test]
fn nullable_sort_keeps_tied_rows_stable_and_bulk_respects_scope_revision_and_confirmation() {
    use financial_gps::{
        ledger_engine::{transactions::LedgerFilter, workflows},
        planning::shared,
    };
    let temp = tempfile::tempdir().unwrap();
    let manager = LedgerManager::new(temp.path()).unwrap();
    let scope = manager.create("main", draft("Bulk", false)).unwrap().scope;
    let store = workflows::PreviewStore::default();
    manager
        .write_domain("main", &scope, |tx| {
            let ctx = Context::system(financial_gps::storage::SqliteSession(tx), &scope)?;
            let id = account(&ctx)?;
            let mut ids = Vec::new();
            for date in [None, None, Some("2026-01-19"), Some("2026-01-19")] {
                let mut posting = tx_draft(&id);
                posting.flagged = date.is_some();
                posting.flag_due_date = date.map(str::to_owned);
                ids.push(transactions::save(&ctx, None, &posting)?.row.id);
            }
            let sort: LedgerFilter = dto(json!({"sort":{"field":"flagDueDate","ascending":true}}));
            let full = transactions::list(&ctx, &sort, &page(), false)?;
            let limit = PageReq {
                cursor: None,
                limit: 1,
            };
            let mut next = limit.clone();
            let mut actual = Vec::new();
            loop {
                let results = transactions::list(&ctx, &sort, &next, false)?;
                actual.extend(results.rows.into_iter().map(|row| row.id));
                if results.next_cursor.is_none() {
                    break;
                }
                next.cursor = results.next_cursor;
            }
            assert_eq!(
                actual,
                full.rows
                    .iter()
                    .map(|row| row.id.clone())
                    .collect::<Vec<_>>()
            );
            let action = |select: &str, action: workflows::BulkAction| {
                workflows::bulk_preview(
                    &ctx,
                    &store,
                    "main",
                    &workflows::IdsOrFilter::Ids {
                        ids: vec![select.into()],
                    },
                    &action,
                )
            };
            let flag = action(
                &ids[0],
                workflows::BulkAction::Flag {
                    flagged: true,
                    due_date: Some("2026-02-01".into()),
                },
            )?;
            assert!(
                workflows::bulk_apply(&ctx, &store, "other", &flag.preview_token, true).is_err()
            );
            assert_eq!(
                workflows::bulk_apply(&ctx, &store, "main", &flag.preview_token, true)?.changed_ids,
                [ids[0].clone()]
            );
            let group = shared::save(&ctx, None, &dto(json!({"name":"Group","kind":"social"})))?;
            let link = action(
                &ids[0],
                workflows::BulkAction::AssignGroup {
                    group_id: group.group.id,
                },
            )?;
            workflows::bulk_apply(&ctx, &store, "main", &link.preview_token, true)?;
            assert!(
                transactions::get(&ctx, &ids[0])?
                    .shared_expense_id
                    .is_some()
            );
            let delete = action(&ids[1], workflows::BulkAction::Delete)?;
            assert!(
                workflows::bulk_apply(&ctx, &store, "main", &delete.preview_token, false).is_err()
            );
            workflows::bulk_apply(&ctx, &store, "main", &delete.preview_token, true)?;
            assert!(transactions::get(&ctx, &ids[1]).is_err());
            let stale = action(
                &ids[2],
                workflows::BulkAction::Flag {
                    flagged: false,
                    due_date: None,
                },
            )?;
            tx.execute("UPDATE ledger_meta SET revision=revision+1", [])
                .map_err(financial_gps::storage::sql_error)?;
            assert_eq!(
                workflows::bulk_apply(&ctx, &store, "main", &stale.preview_token, true)
                    .unwrap_err()
                    .code,
                ErrorCode::StaleRevision
            );
            Ok(())
        })
        .unwrap();
}
