//! Tiny Sails-side client for the kingdoms HTTP director.
//!
//! Drives choose-target / launch-raid / read `/war`, then listens for one
//! webhook and checks `X-Kingdoms-Signature`. Point it at a live server:
//!
//! ```text
//! KINGDOMS_HTTP=1 KINGDOMS_HTTP_TOKEN=<32+ chars> KINGDOMS_HTTP_WEBHOOK=http://127.0.0.1:8743/
//! kingdoms-mock-director --base http://127.0.0.1:8742 --token <same> --town 0,0 --listen 127.0.0.1:8743
//! ```

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines
)]

use std::env;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::post;
use pumpkin_kingdoms::http::{MIN_TOKEN_LEN, verify_signature};
use serde_json::{Value, json};
use tokio::sync::Mutex;

struct Args {
    base: String,
    token: String,
    town: String,
    size: u16,
    defenders: u16,
    listen: Option<String>,
    secret: String,
}

fn arg_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].as_str())
        .or_else(|| {
            args.iter()
                .find_map(|a| a.strip_prefix(&format!("{flag}=")))
        })
}

fn parse_args() -> Result<Args, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!(
            "kingdoms-mock-director --base URL --token TOKEN [--town 0,0] [--size 24] [--defenders 8] [--listen 127.0.0.1:8743] [--secret SECRET]"
        );
        std::process::exit(0);
    }
    let token = arg_value(&args, "--token")
        .map(ToOwned::to_owned)
        .or_else(|| env::var("KINGDOMS_HTTP_TOKEN").ok())
        .ok_or_else(|| "need --token or KINGDOMS_HTTP_TOKEN".to_owned())?;
    if token.len() < MIN_TOKEN_LEN {
        return Err(format!("token shorter than {MIN_TOKEN_LEN} characters"));
    }
    let secret = arg_value(&args, "--secret")
        .map(ToOwned::to_owned)
        .or_else(|| env::var("KINGDOMS_HTTP_WEBHOOK_SECRET").ok())
        .unwrap_or_else(|| token.clone());
    Ok(Args {
        base: arg_value(&args, "--base")
            .unwrap_or("http://127.0.0.1:8742")
            .trim_end_matches('/')
            .to_owned(),
        token,
        town: arg_value(&args, "--town").unwrap_or("0,0").to_owned(),
        size: arg_value(&args, "--size")
            .and_then(|s| s.parse().ok())
            .unwrap_or(24),
        defenders: arg_value(&args, "--defenders")
            .and_then(|s| s.parse().ok())
            .unwrap_or(8),
        listen: arg_value(&args, "--listen").map(ToOwned::to_owned),
        secret,
    })
}

struct HookState {
    secret: String,
    ok: AtomicBool,
    body: Mutex<Option<Value>>,
}

async fn receive_hook(
    State(state): State<Arc<HookState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let header = headers
        .get("x-kingdoms-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !verify_signature(&state.secret, &body, header) {
        eprintln!("webhook HMAC mismatch header={header}");
        return (axum::http::StatusCode::UNAUTHORIZED, "bad signature");
    }
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    println!("webhook {parsed}");
    *state.body.lock().await = Some(parsed);
    state.ok.store(true, Ordering::Relaxed);
    (axum::http::StatusCode::OK, "ok")
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .expect("http client");
    let auth = format!("Bearer {}", args.token);

    let war = match client
        .get(format!("{}/war", args.base))
        .header("Authorization", &auth)
        .send()
        .await
    {
        Ok(r) => r,
        Err(err) => {
            eprintln!("GET /war failed: {err}");
            return ExitCode::from(1);
        }
    };
    if !war.status().is_success() {
        eprintln!("GET /war {}", war.status());
        return ExitCode::from(1);
    }
    let war_json: Value = war.json().await.unwrap_or(json!({}));
    println!(
        "war tick={}",
        war_json.get("tick").and_then(Value::as_u64).unwrap_or(0)
    );

    let towns = war_json
        .get("towns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let chosen = towns
        .iter()
        .find(|t| {
            t.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.ends_with(&format!(":{}", args.town.replace(',', ":"))))
        })
        .or_else(|| towns.first())
        .cloned();
    if let Some(town) = &chosen {
        println!(
            "target {} owner={}",
            town.get("id").and_then(Value::as_str).unwrap_or("?"),
            town.get("owner").and_then(Value::as_str).unwrap_or("?")
        );
    }

    let hook_ok = if let Some(listen) = args.listen.clone() {
        let state = Arc::new(HookState {
            secret: args.secret.clone(),
            ok: AtomicBool::new(false),
            body: Mutex::new(None),
        });
        let listener = match tokio::net::TcpListener::bind(&listen).await {
            Ok(l) => l,
            Err(err) => {
                eprintln!("webhook listen {listen}: {err}");
                return ExitCode::from(1);
            }
        };
        println!("listening for webhooks on {listen}");
        let app = Router::new()
            .route("/", post(receive_hook))
            .with_state(Arc::clone(&state));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Some(state)
    } else {
        None
    };

    let raid = client
        .post(format!("{}/raids", args.base))
        .header("Authorization", &auth)
        .header("content-type", "application/json")
        .json(&json!({
            "town": args.town,
            "size": args.size,
            "defenders": args.defenders,
        }))
        .send()
        .await;
    match raid {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            println!("POST /raids {status} {text}");
            if !status.is_success() && status.as_u16() != 503 && status.as_u16() != 400 {
                return ExitCode::from(1);
            }
        }
        Err(err) => {
            eprintln!("POST /raids failed: {err}");
            return ExitCode::from(1);
        }
    }

    if let Some(state) = hook_ok {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
        while tokio::time::Instant::now() < deadline {
            if state.ok.load(Ordering::Relaxed) {
                println!("webhook HMAC verified");
                return ExitCode::SUCCESS;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        eprintln!("no signed webhook arrived (set KINGDOMS_HTTP_WEBHOOK to this --listen URL)");
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}
