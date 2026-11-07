//! The `run` tool's interpreter: a small, sandboxed script that calls Ostler's own tools.
//!
//! Multi-page jobs ("read these 20 pages, keep the rows that match, report counts") otherwise cost one
//! model turn per step, and every page passes through the model twice. A script does the loop, the
//! filtering and the aggregation outside the model and returns only the answer.
//!
//! The sandbox is the interpreter's own: no file system, no network, no modules, no `eval`, bounded
//! operations, depth, sizes and wall time. The only way out is `tool(name, args)`, which goes back
//! through the same entry point as a direct tool call, so profiles, the domain allowlist, the submit
//! gate, robots.txt and per-tool deadlines all still apply. `run` cannot call `run`.

use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map, Position};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

/// One tool call a script asks for. The reply is the tool's text, or its error text.
pub struct Req {
    pub name: String,
    pub args: Value,
    pub reply: mpsc::Sender<Result<String, String>>,
}

pub struct Limits {
    pub max_calls: usize,
    pub wall: Duration,
    pub max_operations: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_calls: 60,
            wall: Duration::from_secs(110),
            max_operations: 2_000_000,
        }
    }
}

pub struct Outcome {
    /// Everything the script `print`ed, one entry per call.
    pub printed: String,
    /// The script's final value, rendered as JSON text (or plain text for a string); empty for none.
    pub value: String,
    pub calls: usize,
}

fn rt_err(msg: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(Dynamic::from(msg.into()), Position::NONE).into()
}

fn to_json(d: &Dynamic) -> Value {
    rhai::serde::from_dynamic::<Value>(d).unwrap_or_else(|_| Value::String(d.to_string()))
}

/// Run `script`, sending each `tool(...)` to `tx`. Blocks: run it on a blocking thread.
pub fn run(script: &str, tx: UnboundedSender<Req>, limits: Limits) -> Result<Outcome, String> {
    let printed = Arc::new(Mutex::new(String::new()));
    let calls = Arc::new(Mutex::new(0usize));
    let started = Instant::now();

    let mut engine = Engine::new();
    engine.set_max_operations(limits.max_operations);
    engine.set_max_call_levels(32);
    engine.set_max_expr_depths(64, 32);
    engine.set_max_string_size(2_000_000);
    engine.set_max_array_size(20_000);
    engine.set_max_map_size(20_000);
    engine.disable_symbol("eval");
    // `Engine::new` would otherwise let `import "file"` read from disk.
    engine.set_module_resolver(rhai::module_resolvers::DummyModuleResolver::new());

    let wall = limits.wall;
    engine.on_progress(move |_| {
        (started.elapsed() > wall)
            .then(|| Dynamic::from(format!("script ran past {}s", wall.as_secs())))
    });
    let sink = printed.clone();
    engine.on_print(move |s| {
        let mut p = sink.lock().unwrap();
        if p.len() < 200_000 {
            p.push_str(s);
            p.push('\n');
        }
    });

    let count = calls.clone();
    let max_calls = limits.max_calls;
    let call = move |name: &str, args: Dynamic| -> Result<String, Box<EvalAltResult>> {
        {
            let mut n = count.lock().unwrap();
            if *n >= max_calls {
                return Err(rt_err(format!(
                    "a script may make at most {max_calls} tool calls"
                )));
            }
            *n += 1;
        }
        let args = if args.is_unit() {
            Value::Object(Default::default())
        } else {
            to_json(&args)
        };
        let (rtx, rrx) = mpsc::channel();
        tx.send(Req {
            name: name.to_string(),
            args,
            reply: rtx,
        })
        .map_err(|_| rt_err("the session ended"))?;
        match rrx.recv() {
            Ok(Ok(text)) => Ok(text),
            Ok(Err(e)) => Err(rt_err(format!("{name} failed: {e}"))),
            Err(_) => Err(rt_err("the session ended")),
        }
    };
    let c1 = call.clone();
    engine.register_fn("tool", move |name: &str, args: Map| {
        c1(name, Dynamic::from(args))
    });
    engine.register_fn("tool", move |name: &str| call(name, Dynamic::UNIT));
    // Small text helpers: keep the lines of `text` that contain `needle` (case-insensitive).
    engine.register_fn("grep", |text: &str, needle: &str| -> ImmutableString {
        let n = needle.to_lowercase();
        text.lines()
            .filter(|l| l.to_lowercase().contains(&n))
            .collect::<Vec<_>>()
            .join("\n")
            .into()
    });
    engine.register_fn("lines", |text: &str| -> rhai::Array {
        text.lines().map(|l| Dynamic::from(l.to_string())).collect()
    });

    let result = engine.eval::<Dynamic>(script);
    let printed = printed.lock().unwrap().clone();
    let calls = *calls.lock().unwrap();
    match result {
        Ok(v) => {
            let value = if v.is_unit() {
                String::new()
            } else if let Ok(s) = v.clone().into_string() {
                s
            } else {
                to_json(&v).to_string()
            };
            Ok(Outcome {
                printed,
                value,
                calls,
            })
        }
        Err(e) => {
            let msg = match *e {
                EvalAltResult::ErrorRuntime(ref d, _)
                | EvalAltResult::ErrorTerminated(ref d, _) => d.to_string(),
                ref other => other.to_string(),
            };
            Err(if printed.is_empty() {
                msg
            } else {
                format!("{msg}\n(output before the error)\n{printed}")
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Run `script` against a fake tool host that answers each call with `f(name, args)`.
    fn go(
        script: &str,
        limits: Limits,
        f: impl Fn(&str, &Value) -> Result<String, String> + Send + 'static,
    ) -> Result<Outcome, String> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Req>();
        let host = std::thread::spawn(move || {
            while let Some(r) = rx.blocking_recv() {
                let _ = r.reply.send(f(&r.name, &r.args));
            }
        });
        let out = run(script, tx, limits);
        host.join().unwrap();
        out
    }

    #[test]
    fn a_script_loops_calls_tools_and_returns_only_the_answer() {
        let o = go(
            r#"
            let hits = 0;
            for u in ["a", "b", "c"] {
                let t = tool("read", #{url: u});
                if t.contains("ok") { hits += 1; }
            }
            print("done");
            hits
            "#,
            Limits::default(),
            |n, a| {
                assert_eq!(n, "read");
                Ok(if a["url"] == json!("b") {
                    "nope".into()
                } else {
                    "ok page".into()
                })
            },
        )
        .unwrap();
        assert_eq!(o.value, "2");
        assert_eq!(o.printed.trim(), "done");
        assert_eq!(o.calls, 3);
    }

    #[test]
    fn a_failing_tool_aborts_the_script_and_says_which() {
        let e = go(
            r#"tool("navigate", #{url:"x"})"#,
            Limits::default(),
            |_, _| Err("blocked by allowlist".into()),
        )
        .err()
        .unwrap();
        assert!(e.contains("navigate failed: blocked by allowlist"), "{e}");
    }

    #[test]
    fn a_script_can_catch_a_failure_and_carry_on() {
        let o = go(
            r#"let r = "x"; try { tool("navigate"); } catch (e) { r = "caught"; } r"#,
            Limits::default(),
            |_, _| Err("boom".into()),
        )
        .unwrap();
        assert_eq!(o.value, "caught");
    }

    #[test]
    fn the_number_of_tool_calls_is_capped() {
        let l = Limits {
            max_calls: 3,
            ..Limits::default()
        };
        let e = go(r#"loop { tool("links"); }"#, l, |_, _| Ok("x".into()))
            .err()
            .unwrap();
        assert!(e.contains("at most 3 tool calls"), "{e}");
    }

    #[test]
    fn runaway_loops_and_the_wall_clock_are_stopped() {
        let l = Limits {
            max_operations: 10_000,
            ..Limits::default()
        };
        assert!(go("loop { }", l, |_, _| Ok(String::new())).is_err());
        let l = Limits {
            wall: Duration::from_millis(50),
            max_operations: u64::MAX,
            ..Limits::default()
        };
        let e = go("loop { }", l, |_, _| Ok(String::new())).err().unwrap();
        assert!(e.contains("ran past"), "{e}");
    }

    #[test]
    fn there_is_no_eval_no_modules_and_no_files() {
        assert!(go(
            r#"eval("1+1")"#,
            Limits::default(),
            |_, _| Ok(String::new())
        )
        .is_err());
        assert!(go(
            r#"import "/etc/passwd" as p; 1"#,
            Limits::default(),
            |_, _| Ok(String::new())
        )
        .is_err());
        assert!(go(r#"open("/etc/passwd")"#, Limits::default(), |_, _| Ok(
            String::new()
        ))
        .is_err());
    }

    #[test]
    fn grep_and_structured_values_work() {
        let o = go(
            r#"#{ n: lines(grep("a1\nb2\nA3", "a")).len(), tags: ["x"] }"#,
            Limits::default(),
            |_, _| Ok(String::new()),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&o.value).unwrap();
        assert_eq!(v["n"], 2);
        assert_eq!(v["tags"], json!(["x"]));
    }
}
