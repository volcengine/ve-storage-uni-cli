//! Fail-closed validation for one assistant reply and its committed items.
use super::{valid_id, Result, SessionError};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) struct ReplyState<'a> {
    session_id: &'a str,
    user_id: &'a str,
    assistant_id: Option<String>,
    is_completed: bool,
    events: BTreeMap<u64, Value>,
    items: BTreeMap<String, (u64, String)>,
}

impl<'a> ReplyState<'a> {
    pub(super) fn new(session_id: &'a str, user_id: &'a str) -> Self {
        Self {
            session_id,
            user_id,
            assistant_id: None,
            is_completed: false,
            events: BTreeMap::new(),
            items: BTreeMap::new(),
        }
    }

    pub(super) fn accept(&mut self, kind: &str, id: Option<&str>, body: &str) -> Result<()> {
        let value: Value = serde_json::from_str(body).map_err(|_| SessionError::Response)?;
        if !value.is_object() {
            return Err(SessionError::Response);
        }
        if matches!(
            kind,
            "tool_use" | "tool_result" | "artifact" | "subagent.created" | "subagent.closed"
        ) {
            return Err(SessionError::Unsupported);
        }
        if self.is_completed {
            return Err(SessionError::Response);
        }
        match kind {
            "message.created" | "message.in_progress" | "message.completed" => {
                self.lifecycle(kind, &value)?;
                self.record(kind, id, value)
            }
            "message.failed" | "message.cancelled" | "session.failed" | "error" => {
                Err(SessionError::Incomplete)
            }
            "text" | "reasoning" => self.committed(kind, id, value),
            "text.start" | "text.delta" | "reasoning.start" | "reasoning.delta" => {
                self.preview(kind, &value)
            }
            _ => Err(SessionError::Response),
        }
    }

    fn lifecycle(&mut self, kind: &str, value: &Value) -> Result<()> {
        let message_id = field(value, "message_id")?;
        valid_id(message_id)?;
        let message = value.get("message").ok_or(SessionError::Response)?;
        let expected_status = if kind == "message.completed" {
            "completed"
        } else {
            "in_progress"
        };
        if field(value, "session_id")? != self.session_id
            || field(message, "id")? != message_id
            || field(message, "role")? != "assistant"
            || field(message, "status")? != expected_status
            || message_id == self.user_id
            || message.get("items").is_some()
        {
            return Err(SessionError::Response);
        }
        for (key, expected) in [("session_id", self.session_id), ("reply_id", self.user_id)] {
            if message.get(key).is_some() && field(message, key)? != expected {
                return Err(SessionError::Response);
            }
        }
        if self
            .assistant_id
            .as_deref()
            .is_some_and(|current| current != message_id)
        {
            return Err(SessionError::Response);
        }
        if kind == "message.completed" {
            if self.assistant_id.is_none() || self.items.is_empty() {
                return Err(SessionError::Incomplete);
            }
            let seq = sequence(value)?;
            if self
                .events
                .keys()
                .next_back()
                .is_some_and(|previous| *previous >= seq)
            {
                return Err(SessionError::Response);
            }
            self.is_completed = true;
        }
        self.assistant_id = Some(message_id.to_owned());
        Ok(())
    }

    fn committed(&mut self, kind: &str, id: Option<&str>, value: Value) -> Result<()> {
        self.identity(&value)?;
        let item_id = field(&value, "id")?.to_owned();
        valid_id(&item_id)?;
        if field(&value, "type")? != kind {
            return Err(SessionError::Response);
        }
        if kind == "text" {
            let item = (sequence(&value)?, field(&value, "text")?.to_owned());
            if self
                .items
                .get(&item_id)
                .is_some_and(|previous| previous != &item)
            {
                return Err(SessionError::Response);
            }
            self.items.insert(item_id, item);
        }
        self.record(kind, id, value)
    }

    fn preview(&self, kind: &str, value: &Value) -> Result<()> {
        self.identity(value)?;
        valid_id(field(value, "id")?)?;
        if kind.ends_with(".start") {
            let expected = kind.split('.').next().ok_or(SessionError::Response)?;
            if field(value, "type")? != expected {
                return Err(SessionError::Response);
            }
        } else {
            field(value, "delta")?;
        }
        Ok(())
    }

    fn identity(&self, value: &Value) -> Result<()> {
        if self.assistant_id.as_deref() != Some(field(value, "message_id")?) {
            return Err(SessionError::Response);
        }
        Ok(())
    }

    fn record(&mut self, kind: &str, id: Option<&str>, value: Value) -> Result<()> {
        let seq = sequence(&value)?;
        let message_id = field(&value, "message_id")?;
        if id.is_some_and(|id| id != format!("{message_id}:{seq}")) {
            return Err(SessionError::Response);
        }
        if kind.starts_with("message.") && field(&value, "type")? != kind {
            return Err(SessionError::Response);
        }
        if self
            .events
            .get(&seq)
            .is_some_and(|previous| previous != &value)
        {
            return Err(SessionError::Response);
        }
        self.events.insert(seq, value);
        Ok(())
    }

    pub(super) fn finish(self) -> Result<String> {
        if !self.is_completed {
            return Err(SessionError::Incomplete);
        }
        let mut items: Vec<_> = self.items.into_values().collect();
        items.sort_by_key(|(seq, _)| *seq);
        let text: String = items.into_iter().map(|(_, text)| text).collect();
        if text.trim().is_empty() {
            return Err(SessionError::Incomplete);
        }
        Ok(text)
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or(SessionError::Response)
}

fn sequence(value: &Value) -> Result<u64> {
    value
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or(SessionError::Response)
}
