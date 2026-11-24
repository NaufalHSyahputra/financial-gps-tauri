use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use reqwest::{StatusCode, Url, blocking::Client, header::HeaderValue};
use serde_json::{Value, json};
use std::{io::Read, time::Duration};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{Commit, Remote, SyncToken, corrupt};
use crate::domain::{AppError, ErrorCode, Result};

const CHUNK: usize = 256 * 1024;
const RESPONSE_LIMIT: u64 = 512 * 1024;

pub fn endpoint(input: &str) -> Result<Url> {
    let invalid = || AppError::validation("url", "invalid_turso_endpoint");
    if input.len() > 260 {
        return Err(invalid());
    }
    let authority = input
        .strip_prefix("libsql://")
        .or_else(|| input.strip_prefix("https://"))
        .ok_or_else(invalid)?
        .trim_end_matches('/');
    let host = authority.to_ascii_lowercase();
    let prefix = host.strip_suffix(".turso.io").ok_or_else(invalid)?;
    if prefix.is_empty()
        || prefix.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        || input.matches('/').count() > 3
    {
        return Err(invalid());
    }
    Url::parse(&format!("https://{host}/")).map_err(|_| invalid())
}

pub struct Turso {
    client: Client,
    pipeline: Url,
    auth: HeaderValue,
}

impl Turso {
    pub fn connect(url: &str, token: &SyncToken) -> Result<Self> {
        let mut auth = HeaderValue::from_str(&Zeroizing::new(format!("Bearer {}", token.as_str())))
            .map_err(|_| AppError::validation("authToken", "invalid_turso_token"))?;
        auth.set_sensitive(true);
        Ok(Self {
            client: Client::builder()
                .https_only(true)
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|_| offline())?,
            pipeline: endpoint(url)?.join("v3/pipeline").map_err(|_| corrupt())?,
            auth,
        })
    }

    #[cfg(test)]
    pub(super) fn connect_loopback(address: std::net::SocketAddr, token: &SyncToken) -> Self {
        let mut instance = Self::connect("https://test.turso.io", token).unwrap();
        instance.pipeline = Url::parse(&format!("http://{address}/v3/pipeline")).unwrap();
        instance.client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        instance
    }

    fn exec(&self, sql: &str, args: Vec<Value>) -> Result<Value> {
        let response = self
            .client
            .post(self.pipeline.clone())
            .header(reqwest::header::AUTHORIZATION, self.auth.clone())
            .json(&json!({
                "requests":[{"type":"execute","stmt":{"sql":sql,"args":args,"want_rows":true}},{"type":"close"}]
            }))
            .send()
            .map_err(|_| offline())?;
        match response.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(AppError::new(ErrorCode::Validation, "turso_auth_rejected"));
            }
            status if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS => {
                return Err(offline());
            }
            status if !status.is_success() => {
                return Err(AppError::new(ErrorCode::Io, "turso_protocol_rejected"));
            }
            _ => {}
        }
        let mut body = Zeroizing::new(Vec::new());
        response
            .take(RESPONSE_LIMIT + 1)
            .read_to_end(&mut body)
            .map_err(|_| offline())?;
        if body.len() as u64 > RESPONSE_LIMIT {
            return Err(corrupt());
        }
        let mut reply: Value = serde_json::from_slice(&body).map_err(|_| corrupt())?;
        let response = reply
            .get_mut("results")
            .and_then(|rows| rows.get_mut(0))
            .ok_or_else(corrupt)?;
        if response["type"] == "error" {
            let code = response["error"]["code"].as_str().unwrap_or_default();
            return Err(
                if matches!(code, "SQLITE_AUTH" | "SQLITE_PERM" | "SQLITE_READONLY") {
                    AppError::new(ErrorCode::Validation, "turso_auth_rejected")
                } else {
                    AppError::new(ErrorCode::Io, "turso_sql_rejected")
                },
            );
        }
        if response["type"] != "ok" || response["response"]["type"] != "execute" {
            return Err(corrupt());
        }
        Ok(response
            .get_mut("response")
            .and_then(|entry| entry.get_mut("result"))
            .ok_or_else(corrupt)?
            .take())
    }

    fn commit(&self, sql: &str, args: Vec<Value>) -> Result<Option<Commit>> {
        let result = self.exec(sql, args)?;
        let rows = result["rows"].as_array().ok_or_else(corrupt)?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let commit = Commit {
            generation: read_integer(&row[0])?,
            id: read_uuid(&row[1])?,
            device: read_uuid(&row[2])?,
            version: read_integer(&row[3])?,
            digest: row[4]["value"].as_str().ok_or_else(corrupt)?.to_owned(),
            parts: usize::try_from(read_integer(&row[5])?).map_err(|_| corrupt())?,
        };
        if commit.generation <= 0
            || commit.version <= 0
            || commit.parts == 0
            || commit.parts > super::MAX_IMAGE.div_ceil(CHUNK)
            || commit.digest.len() != 64
            || !commit.digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(corrupt());
        }
        Ok(Some(commit))
    }

    fn discard_uncommitted(&self, id: Uuid) -> Result<()> {
        self.exec(
            "DELETE FROM fgps_sync_parts_v1 WHERE id=?1 AND NOT EXISTS(
              SELECT 1 FROM fgps_sync_commits_v1 WHERE id=?1)",
            vec![text(id)],
        )?;
        Ok(())
    }
}

impl Remote for Turso {
    fn probe(&self) -> Result<()> {
        let result = self.exec("SELECT 1", vec![])?;
        if read_integer(&result["rows"][0][0])? != 1 {
            return Err(corrupt());
        }
        Ok(())
    }

    fn initialize(&self) -> Result<()> {
        self.exec(
            "CREATE TABLE IF NOT EXISTS fgps_sync_commits_v1(
              generation INTEGER PRIMARY KEY CHECK(generation>0), id TEXT NOT NULL UNIQUE,
              device TEXT NOT NULL, local_version INTEGER NOT NULL CHECK(local_version>0),
              digest TEXT NOT NULL, parts INTEGER NOT NULL CHECK(parts>0),
              UNIQUE(device,local_version)) STRICT",
            vec![],
        )?;
        self.exec(
            "CREATE TABLE IF NOT EXISTS fgps_sync_parts_v1(
              id TEXT NOT NULL, part INTEGER NOT NULL, payload BLOB NOT NULL,
              PRIMARY KEY(id,part)) STRICT",
            vec![],
        )?;
        Ok(())
    }

    fn head(&self) -> Result<Option<Commit>> {
        self.commit("SELECT generation,id,device,local_version,digest,parts FROM fgps_sync_commits_v1 ORDER BY generation DESC LIMIT 1", vec![])
    }
    fn latest(&self, device: Uuid) -> Result<Option<Commit>> {
        self.commit("SELECT generation,id,device,local_version,digest,parts FROM fgps_sync_commits_v1 WHERE device=?1 ORDER BY local_version DESC LIMIT 1", vec![text(device)])
    }
    fn publish(
        &self,
        base: i64,
        device: Uuid,
        version: i64,
        image: &[u8],
        active: &dyn Fn() -> Result<()>,
    ) -> Result<bool> {
        let id = Uuid::new_v4();
        let publish = || {
            for (index, chunk) in image.chunks(CHUNK).enumerate() {
                active()?;
                self.exec(
                    "INSERT INTO fgps_sync_parts_v1 VALUES(?1,?2,?3)",
                    vec![
                        text(id),
                        integer(index),
                        json!({"type":"blob","base64":STANDARD_NO_PAD.encode(chunk)}),
                    ],
                )?;
            }
            active()?;
            let result = self.exec(
                "INSERT INTO fgps_sync_commits_v1 SELECT ?1+1,?2,?3,?4,?5,?6
                 WHERE ?1=(SELECT coalesce(max(generation),0) FROM fgps_sync_commits_v1)
                 AND ?6=(SELECT count(*) FROM fgps_sync_parts_v1 WHERE id=?2)
                 ON CONFLICT DO NOTHING",
                vec![
                    integer(base),
                    text(id),
                    text(device),
                    integer(version),
                    text(crate::domain::sha256(image)),
                    integer(image.len().div_ceil(CHUNK)),
                ],
            )?;
            Ok(result["affected_row_count"].as_u64().ok_or_else(corrupt)? == 1)
        };
        let result = publish();
        if !matches!(result, Ok(true)) && active().is_ok() {
            let _ = self.discard_uncommitted(id);
        }
        result
    }

    fn pull(&self, commit: &Commit, active: &dyn Fn() -> Result<()>) -> Result<Zeroizing<Vec<u8>>> {
        let mut bytes = Zeroizing::new(Vec::new());
        for index in 0..commit.parts {
            active()?;
            let reply = self.exec(
                "SELECT payload FROM fgps_sync_parts_v1 WHERE id=?1 AND part=?2",
                vec![text(commit.id), integer(index)],
            )?;
            let value = &reply["rows"][0][0];
            if value["type"] != "blob" {
                return Err(corrupt());
            }
            let encoded = value["base64"].as_str().ok_or_else(corrupt)?;
            let chunk = Zeroizing::new(
                STANDARD_NO_PAD
                    .decode(encoded)
                    .or_else(|_| STANDARD.decode(encoded))
                    .map_err(|_| corrupt())?,
            );
            if chunk.is_empty()
                || chunk.len() > CHUNK
                || bytes.len() + chunk.len() > super::MAX_IMAGE
            {
                return Err(corrupt());
            }
            bytes.extend_from_slice(&chunk);
        }
        if crate::domain::sha256(&bytes) != commit.digest {
            return Err(corrupt());
        }
        Ok(bytes)
    }
}

fn text(value: impl ToString) -> Value {
    json!({"type":"text","value":value.to_string()})
}
fn integer(value: impl ToString) -> Value {
    json!({"type":"integer","value":value.to_string()})
}
fn read_integer(value: &Value) -> Result<i64> {
    if value["type"] != "integer" {
        return Err(corrupt());
    }
    value["value"]
        .as_str()
        .and_then(|v| v.parse().ok())
        .ok_or_else(corrupt)
}
fn read_uuid(value: &Value) -> Result<Uuid> {
    value["value"]
        .as_str()
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(corrupt)
}
fn offline() -> AppError {
    AppError::new(ErrorCode::Offline, "turso_unreachable")
}
