//! Review obligations belong to Coordinator; delivery acknowledgements never complete them.
use super::*;
use crate::events::{EventService, NewEvent};
use devcoordinator2_api::results::{OtherOwnedEvent, OwnedEvent};
use devcoordinator2_api::review_policy::{self as api, Policy, Reminder};

pub(crate) enum RegistrationMode {
    Refresh,
    Replace,
}

fn scope_key(workstream: &Option<String>) -> String {
    serde_json::to_string(workstream).expect("string scope encodes")
}
fn identifier(value: &str) -> Result<(), ProtocolError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
    {
        return Err(invalid("Invalid review routing identifier"));
    }
    Ok(())
}
impl ReviewService {
    pub(crate) fn policy_set(&self, p: api::Set, now: u64) -> Result<Policy, ProtocolError> {
        self.repository(&p.repository_id)?;
        if let Some(scope) = &p.workstream_id {
            identifier(scope)?;
        }
        let interval = p.review_interval_ms.unwrap_or(86_400_000);
        let escalation = p.escalation_interval_ms.unwrap_or(3_600_000);
        let repo = p.repository_id.clone();
        let key = scope_key(&p.workstream_id);
        let previous=self.database.call(move|c|{
            let mut q=c.prepare("SELECT record_id,revision,record_json FROM review_records WHERE repository_id=?1 AND json_extract(record_json,'$.workstreamId') IS json_extract(?2,'$') AND json_extract(record_json,'$.experiment.disposition') IN ('retained','reverted','inconclusive','unchanged') ORDER BY window_end_ms DESC,revision DESC LIMIT 32")?;
            let rows=q.query_map(rusqlite::params![repo,key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
            Ok(rows.into_iter().find_map(|(id,revision,json)|serde_json::from_str::<ReviewRecord>(&json).ok().filter(review_completed).map(|record|(record.window_end_ms,format!("{id}@{revision}")))))
        }).map_err(database_error)?;
        let start = match (p.window_start_ms, previous.as_ref().map(|(end, _)| *end)) {
            (Some(start), Some(completed)) => start.max(completed),
            (Some(start), None) => start,
            (None, Some(completed)) => completed,
            (None, None) => now,
        };
        let last_receipt = previous.map(|(_, receipt)| receipt);
        if interval == 0
            || escalation == 0
            || interval > 31_536_000_000
            || escalation > 31_536_000_000
            || start > now
        {
            return Err(invalid("Invalid review policy interval"));
        }
        let repo = p.repository_id.clone();
        let scope = scope_key(&p.workstream_id);
        self.database.call(move|c| {
            c.execute("INSERT INTO review_policies(repository_id,workstream_key,interval_ms,escalation_ms,active,window_start_ms,window_end_ms,last_receipt) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(repository_id,workstream_key) DO UPDATE SET interval_ms=COALESCE(?9,interval_ms),escalation_ms=COALESCE(?10,escalation_ms),active=excluded.active", rusqlite::params![repo,scope,interval as i64,escalation as i64,p.active,start as i64,(start+interval) as i64,last_receipt,p.review_interval_ms.map(|ms|ms as i64),p.escalation_interval_ms.map(|ms|ms as i64)])?;
            Ok(())
        }).map_err(database_error)?;
        self.policy_status(
            api::Scope {
                repository_id: p.repository_id,
                workstream_id: p.workstream_id,
            },
            now,
        )?
        .ok_or_else(|| invalid("Review policy unavailable"))
    }
    pub(crate) fn policy_status(
        &self,
        p: api::Scope,
        now: u64,
    ) -> Result<Option<Policy>, ProtocolError> {
        self.repository(&p.repository_id)?;
        let key = scope_key(&p.workstream_id);
        self.database.call(move|c| c.query_row("SELECT interval_ms,escalation_ms,active,window_start_ms,window_end_ms,last_receipt,lease_expires_at,owner_thread_id FROM review_policies WHERE repository_id=?1 AND workstream_key=?2",rusqlite::params![p.repository_id,key],|r|{
            let end=r.get::<_,i64>(4)? as u64;let escalation=r.get::<_,i64>(1)? as u64;let active:bool=r.get(2)?;
            Ok(Policy {repository_id:p.repository_id.clone(),workstream_id:p.workstream_id.clone(),review_interval_ms:r.get::<_,i64>(0)? as u64,escalation_interval_ms:escalation,active,window_start_ms:r.get::<_,i64>(3)? as u64,window_end_ms:end,last_completed_receipt:r.get(5)?,due:active&&now>=end,escalated:active&&now>=end.saturating_add(escalation),owner_thread_id:r.get(7)?,lease_expires_at:r.get::<_,Option<i64>>(6)?.map(|at|at as u64),delivery_route:if r.get::<_,Option<i64>>(6)?.is_some_and(|expiry|expiry>now as i64){"codex_alarm"}else{"agent_messages"}.into()})
        }).optional().map_err(Into::into)).map_err(database_error)
    }
    pub(crate) fn delivery_register(
        &self,
        p: api::Register,
        now: u64,
        mode: RegistrationMode,
    ) -> Result<Policy, ProtocolError> {
        identifier(&p.owner_thread_id)?;
        identifier(&p.alarm_namespace)?;
        if p.capability_revision != 1
            || p.lease_expires_at <= now
            || p.lease_expires_at > now.saturating_add(172_800_000)
        {
            return Err(invalid("Unsupported or expired alarm capability"));
        }
        self.repository(&p.repository_id)?;
        let repo = p.repository_id.clone();
        let scope = scope_key(&p.workstream_id);
        let changed=self.database.call(move|c| Ok(c.execute("UPDATE review_policies SET owner_thread_id=?1,alarm_namespace=?2,lease_expires_at=?3 WHERE repository_id=?4 AND workstream_key=?5 AND (?6 OR owner_thread_id IS NULL OR owner_thread_id=?1 OR lease_expires_at IS NULL OR lease_expires_at<=?7)",rusqlite::params![p.owner_thread_id,p.alarm_namespace,p.lease_expires_at as i64,repo,scope,matches!(mode,RegistrationMode::Replace),now as i64])?)).map_err(database_error)?;
        let _ = changed;
        self.policy_status(
            api::Scope {
                repository_id: p.repository_id,
                workstream_id: p.workstream_id,
            },
            now,
        )?
        .ok_or_else(|| invalid("Review policy unavailable"))
    }
    pub(crate) fn review_reminders(
        &self,
        events: &EventService,
        now: u64,
    ) -> Result<(), ProtocolError> {
        // Persist each window/stage once. Publication retries use the same outbox key.
        self.database.call(move|c| {
            c.execute("INSERT OR IGNORE INTO review_reminders(repository_id,workstream_key,window_start_ms,window_end_ms,due_at_ms,escalation) SELECT repository_id,workstream_key,window_start_ms,window_end_ms,window_end_ms,0 FROM review_policies WHERE active=1 AND window_end_ms<=?1",[now as i64])?;
            c.execute("INSERT OR IGNORE INTO review_reminders(repository_id,workstream_key,window_start_ms,window_end_ms,due_at_ms,escalation) SELECT repository_id,workstream_key,window_start_ms,window_end_ms,window_end_ms+escalation_ms,1 FROM review_policies WHERE active=1 AND window_end_ms+escalation_ms<=?1",[now as i64])?;
            Ok(())
        }).map_err(database_error)?;
        let pending=self.database.call(move|c| {
            let mut q=c.prepare("SELECT r.reminder_id,r.repository_id,r.workstream_key,r.window_start_ms,r.window_end_ms,r.escalation,p.last_receipt,p.owner_thread_id,p.alarm_namespace,p.lease_expires_at,r.event_route,r.message_id,r.due_at_ms FROM review_reminders r JOIN review_policies p USING(repository_id,workstream_key) WHERE r.resolved=0 AND p.active=1 AND ((p.lease_expires_at>?1 AND (r.event_route IS NULL OR r.event_route != p.owner_thread_id || ':' || p.alarm_namespace)) OR ((p.lease_expires_at IS NULL OR p.lease_expires_at<=?1) AND r.message_id IS NULL)) ORDER BY r.reminder_id LIMIT 128")?;
            let rows=q.query_map([now as i64],|r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,bool>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<String>>(7)?,r.get::<_,Option<String>>(8)?,r.get::<_,Option<i64>>(9)?,r.get::<_,Option<String>>(10)?,r.get::<_,Option<String>>(11)?,r.get::<_,i64>(12)?)))?.collect::<Result<Vec<_>,_>>()?;
            Ok(rows)
        }).map_err(database_error)?;
        for (
            id,
            repo,
            scope,
            start,
            end,
            escalation,
            receipt,
            owner,
            namespace,
            expiry,
            sent,
            message,
            due_at_ms,
        ) in pending
        {
            let workstream =
                serde_json::from_str(&scope).map_err(|_| invalid("Invalid stored review scope"))?;
            if let (Some(owner), Some(namespace), Some(expiry)) = (owner, namespace, expiry)
                && expiry > now as i64
            {
                let route = format!("{owner}:{namespace}");
                if sent.as_ref() == Some(&route) {
                    continue;
                }
                let reminder = Reminder {
                    version: 1,
                    reminder_id: format!("review-{id}"),
                    repository_id: repo.clone(),
                    workstream_id: workstream,
                    owner_thread_id: owner,
                    alarm_namespace: namespace,
                    window_start_ms: start as u64,
                    window_end_ms: end as u64,
                    due_at_ms: due_at_ms as u64,
                    last_completed_receipt: receipt,
                    escalation,
                };
                let event = OtherOwnedEvent {
                    kind: "review.reminder".into(),
                    repository_id: Some(repo),
                    deployment_id: None,
                    subject_kind: "review".into(),
                    subject_id: reminder.reminder_id.clone(),
                    review: Some(reminder),
                };
                events.publish(NewEvent {
                    occurred_at: OffsetDateTime::from_unix_timestamp_nanos(
                        i128::from(now) * 1_000_000,
                    )
                    .map_err(|_| invalid("Invalid review time"))?
                    .format(&Rfc3339)
                    .map_err(|_| invalid("Invalid review time"))?,
                    event: OwnedEvent::Other(event),
                    dedupe_key: Some(format!("review-{id}:{route}")),
                })?;
                self.database
                    .call(move |c| {
                        c.execute(
                            "UPDATE review_reminders SET event_route=?1 WHERE reminder_id=?2",
                            rusqlite::params![route, id],
                        )?;
                        Ok(())
                    })
                    .map_err(database_error)?;
            } else if message.is_none() {
                let message_id = crate::ids::agent_message_id()
                    .map_err(|_| invalid("Cannot allocate reminder"))?;
                let summary = format!(
                    "Performance review {} for {} / {}: window [{start},{end}). Last receipt: {}. Inspect bounded usage evidence; use review.prepare then review.record. A message acknowledgement is not completion. {}",
                    if escalation { "escalation" } else { "due" },
                    repo,
                    scope,
                    receipt.as_deref().unwrap_or("none"),
                    if escalation {
                        "Complete the review or obtain an explicit user override; override leaves it outstanding."
                    } else {
                        "Act before unrelated work."
                    }
                );
                self.database.transaction(move|tx|{let claimed=tx.execute("UPDATE review_reminders SET message_id=?1 WHERE reminder_id=?2 AND message_id IS NULL AND resolved=0",rusqlite::params![message_id,id])?;if claimed==0{return Ok(());}tx.execute("INSERT INTO agent_messages(message_id,repository_id,kind,subject_id,summary,created_at) VALUES(?1,?2,'performance_review.reminder',?3,?4,?5)",rusqlite::params![message_id,repo,format!("review-{id}"),summary,OffsetDateTime::from_unix_timestamp_nanos(i128::from(now)*1_000_000).unwrap().format(&Rfc3339).unwrap()])?;tx.execute("UPDATE review_reminders SET message_id=?1 WHERE reminder_id=?2",rusqlite::params![message_id,id])?;Ok(())}).map_err(database_error)?;
            }
        }
        Ok(())
    }
}

pub(super) fn complete(
    tx: &rusqlite::Transaction<'_>,
    revision: &Revision,
) -> Result<(), crate::database::DatabaseError> {
    if !revision.completed {
        return Ok(());
    }
    let r = &revision.record;
    let key = scope_key(&r.workstream_id);
    tx.execute("UPDATE review_policies SET window_start_ms=?1,window_end_ms=?1+interval_ms,last_receipt=?2 WHERE repository_id=?3 AND workstream_key=?4 AND window_start_ms>=?5 AND window_end_ms<=?1",rusqlite::params![r.window_end_ms as i64,revision.reference,r.repository_id,key,r.window_start_ms as i64])?;
    tx.execute("UPDATE review_reminders SET resolved=1 WHERE repository_id=?1 AND workstream_key=?2 AND window_start_ms>=?3 AND window_end_ms<=?4",rusqlite::params![r.repository_id,key,r.window_start_ms as i64,r.window_end_ms as i64])?;
    // A message receipt records transport handling only; resolution is separate.
    Ok(())
}

impl ReviewService {
    /// Reconcile outstanding windows after source cursor expiry or runtime restart.
    pub(crate) fn delivery_pending(
        &self,
        p: api::PendingRequest,
        now: u64,
    ) -> Result<api::Pending, ProtocolError> {
        identifier(&p.alarm_namespace)?;
        self.database.call(move|c|{
            let cursor=c.query_row("SELECT COALESCE(MAX(cursor),0) FROM owned_events",[],|r|r.get::<_,i64>(0))? as u64;
            let mut query=c.prepare("SELECT r.reminder_id,r.repository_id,r.workstream_key,r.window_start_ms,r.window_end_ms,r.escalation,p.last_receipt,p.owner_thread_id,p.alarm_namespace,r.due_at_ms FROM review_reminders r JOIN review_policies p USING(repository_id,workstream_key) WHERE r.resolved=0 AND p.active=1 AND p.lease_expires_at>?1 AND p.alarm_namespace=?2 AND r.reminder_id>?3 ORDER BY r.reminder_id LIMIT 17")?;
            let rows=query.query_map(rusqlite::params![now as i64,p.alarm_namespace,p.after_id as i64],|r|{
                let id=r.get::<_,i64>(0)? as u64;
                Ok((id,Reminder{version:1,reminder_id:format!("review-{id}"),repository_id:r.get(1)?,workstream_id:serde_json::from_str(&r.get::<_,String>(2)?).map_err(|e|rusqlite::Error::FromSqlConversionFailure(2,rusqlite::types::Type::Text,Box::new(e)))?,window_start_ms:r.get::<_,i64>(3)? as u64,window_end_ms:r.get::<_,i64>(4)? as u64,due_at_ms:r.get::<_,i64>(9)? as u64,escalation:r.get(5)?,last_completed_receipt:r.get(6)?,owner_thread_id:r.get(7)?,alarm_namespace:r.get(8)?}))
            })?.collect::<Result<Vec<_>,_>>()?;
            let next_after_id=(rows.len()>16).then(||rows[15].0);
            Ok(api::Pending{cursor,reminders:rows.into_iter().take(16).map(|(_,reminder)|reminder).collect(),next_after_id})
        }).map_err(database_error)
    }
}
