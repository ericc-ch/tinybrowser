//! CLI over CDP: create, list, select, evaluate, navigate, close.

use std::io::{self, Write};
use std::process::ExitCode;

use browser::Profile;
use serde_json::{Value, json};

use crate::daemon;

pub fn run(profile: &Profile, args: &[String]) -> ExitCode {
    match run_inner(profile, args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run_inner(profile: &Profile, rest: &[String]) -> io::Result<()> {
    let data_home = daemon::data_home()?;
    let endpoint = daemon::ensure(profile, &data_home)?;
    let mut client = cdp::Client::connect(endpoint.addr())?;
    match rest {
        [] => list(&mut client),
        [cmd] if cmd == "list" => list(&mut client),
        [cmd] if cmd == "create" => create(&mut client, profile, "about:blank"),
        [cmd, url] if cmd == "create" => create(&mut client, profile, url),
        [cmd, id] if cmd == "select" => daemon::write_selected(profile, id),
        [cmd, script] if cmd == "eval" || cmd == "evaluate" => {
            evaluate(&mut client, profile, script)
        }
        [cmd, url] if cmd == "navigate" => navigate(&mut client, profile, url),
        [cmd] if cmd == "close" => close(&mut client, profile, None),
        [cmd, id] if cmd == "close" => close(&mut client, profile, Some(id)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, crate::USAGE)),
    }
}

fn create(client: &mut cdp::Client, profile: &Profile, url: &str) -> io::Result<()> {
    let result = client.call("Target.createTarget", &json!({"url": url}), None)?;
    let id = result
        .get("targetId")
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other("createTarget missing targetId"))?;
    daemon::write_selected(profile, id)?;
    writeln!(io::stdout(), "{id}")?;
    Ok(())
}

fn list(client: &mut cdp::Client) -> io::Result<()> {
    let result = client.call("Target.getTargets", &json!({}), None)?;
    let Some(infos) = result.get("targetInfos").and_then(Value::as_array) else {
        return Ok(());
    };
    for info in infos {
        let id = info.get("targetId").and_then(Value::as_str).unwrap_or("-");
        let url = info.get("url").and_then(Value::as_str).unwrap_or("");
        writeln!(io::stdout(), "{id}\t{url}")?;
    }
    Ok(())
}

fn evaluate(client: &mut cdp::Client, profile: &Profile, script: &str) -> io::Result<()> {
    let session = attach_selected(client, profile)?;
    let result = client.call(
        "Runtime.evaluate",
        &json!({"expression": script}),
        Some(&session),
    )?;
    if let Some(text) = result
        .pointer("/exceptionDetails/text")
        .and_then(Value::as_str)
    {
        return Err(io::Error::other(text.to_owned()));
    }
    let preview = result.get("result").cloned().unwrap_or(Value::Null);
    match preview.get("value") {
        Some(value) => writeln!(io::stdout(), "{value}")?,
        None => writeln!(
            io::stdout(),
            "{}",
            preview.get("type").unwrap_or(&json!("undefined"))
        )?,
    }
    Ok(())
}

fn navigate(client: &mut cdp::Client, profile: &Profile, url: &str) -> io::Result<()> {
    let session = attach_selected(client, profile)?;
    client.call("Page.navigate", &json!({"url": url}), Some(&session))?;
    Ok(())
}

fn close(client: &mut cdp::Client, profile: &Profile, id: Option<&str>) -> io::Result<()> {
    let target = match id {
        Some(id) => id.to_owned(),
        None => selected_target(profile)?,
    };
    client.call("Target.closeTarget", &json!({"targetId": target}), None)?;
    let selected = daemon::read_selected(profile)?;
    if selected.as_deref() != Some(target.as_str()) {
        return Ok(());
    }
    let listed = client.call("Target.getTargets", &json!({}), None)?;
    let next = listed
        .get("targetInfos")
        .and_then(Value::as_array)
        .and_then(|infos| infos.first())
        .and_then(|info| info.get("targetId"))
        .and_then(Value::as_str);
    match next {
        Some(id) => daemon::write_selected(profile, id),
        None => daemon::clear_selected(profile),
    }
}

fn attach_selected(client: &mut cdp::Client, profile: &Profile) -> io::Result<String> {
    let target = selected_target(profile)?;
    let attached = client.call(
        "Target.attachToTarget",
        &json!({"targetId": target, "flatten": true}),
        None,
    )?;
    attached
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("attachToTarget missing sessionId"))
}

fn selected_target(profile: &Profile) -> io::Result<String> {
    daemon::read_selected(profile)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| io::Error::other("no selected target; run create or select"))
}
