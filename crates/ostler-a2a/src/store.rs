//! Task and session bookkeeping.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use ostler_mcp::App;
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio::task::AbortHandle;

use crate::proto;

/// One remote conversation: a browser session plus a lock so its tasks run one at a time.
pub struct Ctx {
    pub app: Arc<App>,
    pub run_lock: tokio::sync::Mutex<()>,
    pub last: std::sync::Mutex<Instant>,
}

impl Ctx {
    pub fn new(app: App) -> Self {
        Self {
            app: Arc::new(app),
            run_lock: tokio::sync::Mutex::new(()),
            last: std::sync::Mutex::new(Instant::now()),
        }
    }

    pub fn touch(&self) {
        *self.last.lock().unwrap() = Instant::now();
    }

    pub fn idle_for(&self) -> std::time::Duration {
        self.last.lock().unwrap().elapsed()
    }
}

pub struct TaskRec {
    pub id: String,
    pub context_id: String,
    pub state: &'static str,
    pub timestamp: String,
    /// Fixed wording written by this server, never page text.
    pub status_text: String,
    pub artifacts: Vec<Value>,
    pub history: Vec<Value>,
    /// The caller asked for a submission at some point; a later `{confirm: true}` completes it.
    pub want_submit: bool,
    pub tx: watch::Sender<u64>,
    pub abort: Option<AbortHandle>,
}

impl TaskRec {
    pub fn new(context_id: &str) -> Self {
        let (tx, _) = watch::channel(0);
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            context_id: context_id.to_string(),
            state: proto::SUBMITTED,
            timestamp: proto::now_rfc3339(),
            status_text: String::new(),
            artifacts: vec![],
            history: vec![],
            want_submit: false,
            tx,
            abort: None,
        }
    }

    pub fn set_state(&mut self, state: &'static str, text: &str) {
        self.state = state;
        self.status_text = text.to_string();
        self.timestamp = proto::now_rfc3339();
        self.tx.send_modify(|v| *v += 1);
    }

    pub fn status(&self) -> Value {
        let mut s = json!({ "state": self.state, "timestamp": self.timestamp });
        if !self.status_text.is_empty() {
            s["message"] =
                proto::agent_message(&self.status_text, &self.context_id, Some(&self.id));
        }
        s
    }

    pub fn json(&self, history_length: Option<usize>) -> Value {
        let mut t = json!({
            "id": self.id,
            "contextId": self.context_id,
            "status": self.status(),
        });
        if !self.artifacts.is_empty() {
            t["artifacts"] = json!(self.artifacts);
        }
        let n = history_length.unwrap_or(0).min(self.history.len());
        if n > 0 {
            t["history"] = json!(self.history[self.history.len() - n..]);
        }
        t
    }
}

pub struct TaskStore {
    pub map: HashMap<String, TaskRec>,
    order: VecDeque<String>,
    cap: usize,
}

impl TaskStore {
    pub fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            cap,
        }
    }

    /// Insert a task, evicting the oldest finished one if full. Fails when every slot is still live.
    pub fn insert(&mut self, rec: TaskRec) -> Result<(), &'static str> {
        if self.map.len() >= self.cap {
            let victim = self
                .order
                .iter()
                .find(|id| {
                    self.map
                        .get(*id)
                        .is_some_and(|t| proto::is_terminal(t.state))
                })
                .cloned();
            match victim {
                Some(v) => {
                    self.map.remove(&v);
                    self.order.retain(|x| x != &v);
                }
                None => return Err("too many unfinished tasks"),
            }
        }
        self.order.push_back(rec.id.clone());
        self.map.insert(rec.id.clone(), rec);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_store_evicts_finished_tasks_but_never_live_ones() {
        let mut s = TaskStore::new(2);
        let mut a = TaskRec::new("c");
        a.set_state(proto::COMPLETED, "");
        let a_id = a.id.clone();
        s.insert(a).unwrap();
        let mut b = TaskRec::new("c");
        b.set_state(proto::WORKING, "");
        let b_id = b.id.clone();
        s.insert(b).unwrap();
        s.insert(TaskRec::new("c")).unwrap();
        assert!(
            !s.map.contains_key(&a_id),
            "finished task is the one evicted"
        );
        assert!(s.map.contains_key(&b_id));
        // Now both slots hold unfinished tasks.
        assert!(s.insert(TaskRec::new("c")).is_err());
    }

    #[test]
    fn status_text_is_attached_as_an_agent_message() {
        let mut t = TaskRec::new("ctx1");
        t.set_state(proto::INPUT_REQUIRED, "need email");
        let j = t.json(None);
        assert_eq!(j["status"]["state"], "TASK_STATE_INPUT_REQUIRED");
        assert_eq!(j["status"]["message"]["role"], "ROLE_AGENT");
        assert_eq!(j["status"]["message"]["parts"][0]["text"], "need email");
        assert_eq!(j["contextId"], "ctx1");
    }
}
