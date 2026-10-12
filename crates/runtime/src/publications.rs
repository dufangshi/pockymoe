//! Explicitly published transcripts. A capability can read only the owner's
//! selected turns and turns created after publication, never a private DTO.
use crate::Supervisor;
use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng, Payload},
    Aes256Gcm, Nonce,
};
use anyhow::{ensure, Result};
use base64::Engine;
use pockymoe_protocol::{now_rfc3339, ThreadTurnDto};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePublication {
    pub turn_ids: Vec<String>,
    pub theme: String,
    // Delivered only via the encrypted owner channel, never to the relay.
    pub encryption_key: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Publication {
    thread_id: String,
    turn_ids: HashSet<String>,
    after_ordinal: i64,
    theme: String,
    created_at: String,
    // None is only for already-stored plaintext v1 publication capabilities.
    #[serde(default)]
    encryption_key: Option<String>,
}

/// Binding removes legacy capabilities in the same transaction as the trust pin.
pub fn delete_keyless_publications(conn: &rusqlite::Connection) -> Result<()> {
    let rows = conn
        .prepare("SELECT key,value FROM kv WHERE key LIKE 'publication:%'")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (key, raw) in rows {
        let policy: Publication = serde_json::from_str(&raw)?;
        if policy.encryption_key.is_none() {
            conn.execute("DELETE FROM kv WHERE key=?1", [key])?;
        }
    }
    Ok(())
}

impl Supervisor {
    pub async fn create_publication(
        &self,
        thread: &str,
        input: CreatePublication,
    ) -> Result<Value> {
        ensure!(
            !input.turn_ids.is_empty() && input.turn_ids.len() <= 10_000,
            "Select at least one turn"
        );
        decode_key(&input.encryption_key)?;
        self.get_thread(thread)?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let policy = self.db.with(|conn| {
            let mut stmt =
                conn.prepare("SELECT id,ordinal FROM thread_turns WHERE thread_id=?1")?;
            let rows = stmt
                .query_map([thread], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(
                input
                    .turn_ids
                    .iter()
                    .all(|id| rows.iter().any(|(known, _)| known == id)),
                "Selected turn not found"
            );
            Ok(Publication {
                thread_id: thread.into(),
                turn_ids: input.turn_ids.into_iter().collect(),
                after_ordinal: rows.iter().map(|(_, n)| *n).max().unwrap_or(0),
                theme: if input.theme == "light" {
                    "light"
                } else {
                    "dark"
                }
                .into(),
                created_at: now_rfc3339(),
                encryption_key: Some(input.encryption_key),
            })
        })?;
        self.db.with(|conn| {
            conn.execute(
                "INSERT INTO kv(key,value) VALUES (?1,?2)",
                params![
                    format!("publication:{token}"),
                    serde_json::to_string(&policy)?
                ],
            )?;
            Ok(())
        })?;
        match self.read_publication(&token).await {
            Ok(snapshot) => Ok(json!({"token":token,"snapshot":snapshot})),
            Err(error) => {
                self.revoke_publication(&token)?;
                Err(error)
            }
        }
    }

    pub fn revoke_publication(&self, token: &str) -> Result<()> {
        uuid::Uuid::parse_str(token)?;
        self.db.with(|conn| {
            conn.execute(
                "DELETE FROM kv WHERE key=?1",
                [format!("publication:{token}")],
            )?;
            Ok(())
        })
    }

    pub async fn read_publication(&self, token: &str) -> Result<Value> {
        uuid::Uuid::parse_str(token)?;
        let policy: Publication = self.db.with(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT value FROM kv WHERE key=?1",
                    [format!("publication:{token}")],
                    |r| r.get(0),
                )
                .optional()?;
            let policy: Publication = serde_json::from_str(
                &raw.ok_or_else(|| anyhow::anyhow!("Publication not found"))?,
            )?;
            let bound: bool = conn.query_row(
                "SELECT account_key_id IS NOT NULL FROM device_owner_trust WHERE singleton=1",
                [],
                |r| r.get(0),
            )?;
            ensure!(
                !bound || policy.encryption_key.is_some(),
                "Publication not found"
            );
            Ok(policy)
        })?;
        let ids = self.db.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id,ordinal FROM thread_turns WHERE thread_id=?1 ORDER BY ordinal",
            )?;
            let rows = stmt
                .query_map([&policy.thread_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows
                .into_iter()
                .filter(|(id, n)| policy.turn_ids.contains(id) || *n > policy.after_ordinal)
                .map(|(id, _)| id)
                .collect::<Vec<_>>())
        })?;
        ensure!(ids.len() <= 10_000, "Published transcript is too large");
        let detail = self.get_thread_detail(&policy.thread_id, None).await?;
        let title = detail.thread.title;
        let ids: HashSet<_> = ids.into_iter().collect();
        let mut turns = Vec::new();
        let mut images = serde_json::Map::new();
        let mut image_bytes = 0;
        for turn in detail
            .turns
            .into_iter()
            .filter(|turn| ids.contains(&turn.id))
        {
            let projected = project_turn(&turn);
            for message in projected["messages"].as_array().unwrap() {
                if message["role"] != "user" {
                    continue;
                }
                for tail in message["text"]
                    .as_str()
                    .unwrap_or("")
                    .split("[PHOTO ")
                    .skip(1)
                {
                    let Some((path, _)) = tail.split_once(']') else {
                        continue;
                    };
                    let path = path.trim();
                    if images.contains_key(path) {
                        continue;
                    }
                    let (bytes, mime) = self.thread_image(&policy.thread_id, path)?;
                    image_bytes += bytes.len();
                    ensure!(
                        image_bytes <= 10 * 1024 * 1024,
                        "Published images exceed 10 MB"
                    );
                    images.insert(
                        path.into(),
                        json!(format!(
                            "data:{mime};base64,{}",
                            base64::engine::general_purpose::STANDARD.encode(bytes)
                        )),
                    );
                }
            }
            turns.push(projected);
        }
        let snapshot = json!({"title":title,"createdAt":policy.created_at,"updatedAt":now_rfc3339(),"live":true,"turnCount":turns.len(),"turns":turns,"theme":policy.theme,"images":images});
        ensure!(
            snapshot.to_string().len() <= 16 * 1024 * 1024,
            "Published transcript exceeds 16 MB"
        );
        match policy.encryption_key {
            Some(key) => encrypt_snapshot(&snapshot, &key),
            None => {
                // Binding can happen through a separate offline SQLite connection
                // while this async read is in flight.
                self.db.with(|c| {
                    let bound: bool = c.query_row("SELECT account_key_id IS NOT NULL FROM device_owner_trust WHERE singleton=1", [], |r|r.get(0))?;
                    ensure!(!bound,"Publication not found");
                    Ok(())
                })?;
                Ok(snapshot)
            }
        }
    }
}

fn project_turn(turn: &ThreadTurnDto) -> Value {
    let final_message = if turn.status == "inProgress" {
        None
    } else {
        turn.items.iter().rfind(|i| {
            i.kind == "agentMessage"
                && i.extra.get("phase").and_then(Value::as_str) != Some("commentary")
        })
    };
    let messages: Vec<_> = turn.items.iter().filter(|i| i.kind == "userMessage" || final_message.is_some_and(|last| std::ptr::eq(*i, last))).map(|i| json!({"role":if i.kind == "userMessage" {"user"} else {"assistant"},"text":i.text,"createdAt":i.created_at})).collect();
    let usage = turn.token_usage.as_ref().map(|value| {
        let fields = ["totalTokens", "inputTokens", "cachedInputTokens", "cacheWriteInputTokens", "cacheWriteOneHourInputTokens", "outputTokens", "reasoningOutputTokens"];
        json!({"total":public_numbers(&value["total"], &fields),"last":public_numbers(&value["last"], &fields),"modelContextWindow":value["modelContextWindow"].as_u64()})
    });
    let price = turn.price_estimate.as_ref().map(|value| {
        let mut price = public_numbers(
            value,
            &[
                "inputUsd",
                "cachedInputUsd",
                "cacheWriteInputUsd",
                "outputUsd",
                "totalUsd",
            ],
        );
        for key in ["pricingModelKey", "pricingTierKey", "currency"] {
            price[key] = json!(value[key].as_str());
        }
        price
    });
    json!({"messages":messages,"startedAt":turn.started_at,"completedAt":turn.completed_at,"model":turn.model,"reasoningEffort":turn.reasoning_effort,"tokenUsage":usage,"priceEstimate":price})
}

fn public_numbers(source: &Value, fields: &[&str]) -> Value {
    Value::Object(
        fields
            .iter()
            .filter_map(|key| {
                source[*key]
                    .as_f64()
                    .filter(|n| *n >= 0.0)
                    .map(|n| ((*key).into(), json!(n)))
            })
            .collect(),
    )
}

const PUBLIC_LINK_AAD: &[u8] = b"pockymoe-public-link-v1";

fn decode_key(key: &str) -> Result<Vec<u8>> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(key)
        .map_err(|_| anyhow::anyhow!("Invalid public link encryption key"))?;
    ensure!(bytes.len() == 32, "Invalid public link encryption key");
    Ok(bytes)
}

// Encrypt the entire projection, including title, theme and embedded images.
// Each read gets a fresh IV; AES-GCM's appended tag and base64url encoding match
// browser WebCrypto. The URL fragment contains the key, never the relay request.
fn encrypt_snapshot(snapshot: &Value, key: &str) -> Result<Value> {
    let cipher = Aes256Gcm::new_from_slice(&decode_key(key)?)
        .map_err(|_| anyhow::anyhow!("Invalid public link encryption key"))?;
    let mut iv = [0u8; 12];
    OsRng.fill_bytes(&mut iv);
    let plaintext = serde_json::to_vec(snapshot)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&iv),
            Payload {
                msg: &plaintext,
                aad: PUBLIC_LINK_AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("Unable to encrypt public transcript"))?;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    Ok(json!({"version":2,"iv":b64.encode(iv),"ciphertext":b64.encode(ciphertext)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_crypto_uses_webcrypto_envelope_and_authenticates_every_field() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/public-link-v2.json"))
                .unwrap();
        let key = fixture["key"].as_str().unwrap();
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let cipher = Aes256Gcm::new_from_slice(&decode_key(key).unwrap()).unwrap();
        let decrypt = |envelope: &Value, aad: &[u8]| {
            let iv = b64.decode(envelope["iv"].as_str().unwrap()).unwrap();
            let ciphertext = b64
                .decode(envelope["ciphertext"].as_str().unwrap())
                .unwrap();
            cipher.decrypt(
                Nonce::from_slice(&iv),
                Payload {
                    msg: &ciphertext,
                    aad,
                },
            )
        };
        assert_eq!(
            serde_json::from_slice::<Value>(
                &decrypt(&fixture["envelope"], PUBLIC_LINK_AAD).unwrap()
            )
            .unwrap(),
            fixture["snapshot"]
        );
        let encrypted = encrypt_snapshot(&fixture["snapshot"], key).unwrap();
        assert_eq!(encrypted.as_object().unwrap().len(), 3);
        assert_eq!(
            serde_json::from_slice::<Value>(&decrypt(&encrypted, PUBLIC_LINK_AAD).unwrap())
                .unwrap(),
            fixture["snapshot"]
        );
        assert!(decrypt(&encrypted, b"wrong domain").is_err());
        let mut tampered = encrypted;
        let mut bytes = b64
            .decode(tampered["ciphertext"].as_str().unwrap())
            .unwrap();
        bytes[0] ^= 1;
        tampered["ciphertext"] = json!(b64.encode(bytes));
        assert!(decrypt(&tampered, PUBLIC_LINK_AAD).is_err());
        for invalid in ["", "bad+/=", "AA"] {
            assert!(decode_key(invalid).is_err());
        }
    }
}
