#![forbid(unsafe_code)]
pub mod documents;
pub mod journal;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, Default)]
pub struct AgentConfig {
    pub model: String,
    pub instructions: String,
}
#[derive(Debug, Clone)]
pub enum AgentCommand {
    Open {
        key: u64,
        workspace: PathBuf,
        resume: Option<String>,
        config: AgentConfig,
    },
    Send {
        key: u64,
        prompt: String,
    },
    Cancel(u64),
    Decide {
        request: u64,
        allow: bool,
    },
    Models,
}
#[derive(Debug, Clone)]
pub enum AgentEvent {
    Stopped,
    Status(String),
    Models(Vec<String>),
    Ready {
        key: u64,
        id: String,
    },
    Delta {
        key: u64,
        text: String,
    },
    Activity {
        key: u64,
        kind: String,
    },
    Permission {
        key: u64,
        request: u64,
        description: String,
    },
    Finished {
        key: u64,
    },
    Error {
        key: u64,
        message: String,
    },
    SavedSession {
        id: String,
    },
}
#[derive(Debug, Default)]
pub struct LiveSession {
    pub sdk_id: Option<String>,
    pub draft: String,
    pub config: AgentConfig,
    pub transcript: String,
    pub busy: bool,
    pub scroll_y: f64,
}
impl LiveSession {
    pub fn append(&mut self, text: &str) {
        // Bounded resident transcript; SDK owns the full session history.
        const LIMIT: usize = 1_048_576;
        if self.transcript.len() + text.len() > LIMIT {
            self.transcript.clear();
            self.transcript
                .push_str("[Older output remains in SDK session history.]\n");
        }
        let start = text.len().saturating_sub(LIMIT / 2);
        let start = (start..=text.len())
            .find(|&i| text.is_char_boundary(i))
            .unwrap();
        self.transcript.push_str(&text[start..]);
    }
}
#[derive(Default)]
pub struct LiveWorkspace {
    pub sessions: BTreeMap<u64, LiveSession>,
    pub active: u64,
    pub models: Vec<String>,
    pub saved: Vec<String>,
}
impl LiveWorkspace {
    pub fn add(&mut self) -> u64 {
        let key = self.sessions.last_key_value().map_or(1, |(id, _)| id + 1);
        self.sessions.insert(key, LiveSession::default());
        self.active = key;
        key
    }
    pub fn apply(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Models(models) => self.models = models.clone(),
            AgentEvent::SavedSession { id } => {
                if !self.saved.contains(id) {
                    self.saved.push(id.clone());
                }
            }
            AgentEvent::Ready { key, id } => {
                let s = self.sessions.entry(*key).or_default();
                s.sdk_id = Some(id.clone());
                s.busy = false;
                s.append("[Session connected]\n");
            }
            AgentEvent::Delta { key, text } => self.sessions.entry(*key).or_default().append(text),
            AgentEvent::Activity { key, kind } => self
                .sessions
                .entry(*key)
                .or_default()
                .append(&format!("\n[{kind}]\n")),
            AgentEvent::Error { key, message } => {
                let s = self.sessions.entry(*key).or_default();
                s.busy = false;
                s.append(&format!("\nError: {message}\n"));
            }
            AgentEvent::Finished { key } => {
                let s = self.sessions.entry(*key).or_default();
                s.busy = false;
                s.append("\n[Turn finished]\n");
            }
            _ => {}
        }
    }
}
/// Native frontends depend on this boundary, not SDK/runtime types.
pub trait AgentBackend {
    fn shutdown(&self);
    fn submit(&self, command: AgentCommand) -> Result<(), String>;
    fn try_event(&mut self) -> Option<AgentEvent>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_does_not_recreate_session() {
        let mut w = LiveWorkspace::default();
        let a = w.add();
        w.sessions.get_mut(&a).unwrap().draft = "🦀 draft".into();
        w.add();
        w.active = a;
        w.apply(&AgentEvent::Delta {
            key: a,
            text: "こんにちは".into(),
        });
        assert_eq!(w.sessions[&a].draft, "🦀 draft");
        assert_eq!(w.sessions[&a].transcript, "こんにちは");
    }
    #[test]
    fn oversized_transcript_is_bounded_on_utf8_boundary() {
        let mut s = LiveSession::default();
        s.append(&"🦀".repeat(1_000_000));
        assert!(s.transcript.len() < 1_048_576);
    }
}
