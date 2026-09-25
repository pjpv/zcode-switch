
use crate::store::*;
use serde_json::{json, Value};
use std::fs;

fn flag(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
    }
    None
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn parse_bool(v: &str) -> bool {
    matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

fn resolve_password(rest: &[String]) -> Option<String> {
    if let Some(p) = flag(rest, "--password") {
        return Some(p);
    }
    std::env::var("ZSW_PASSWORD").ok().filter(|p| !p.is_empty())
}

fn ok(v: Value) -> String {
    let mut m = v.as_object().cloned().unwrap_or_default();
    m.insert("ok".into(), Value::Bool(true));
    serde_json::to_string_pretty(&Value::Object(m)).unwrap()
}

fn err(e: &str) -> String {
    serde_json::to_string_pretty(&json!({ "ok": false, "error": e })).unwrap()
}

pub fn run(args: &[String]) -> (String, i32) {
    let mut lang_override: Option<crate::i18n::Lang> = None;
    let args: Vec<String> = {
        let mut out = vec![];
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--lang" {
                if let Some(l) = it.next().and_then(|v| crate::i18n::Lang::parse(v)) {
                    lang_override = Some(l);
                }
            } else {
                out.push(a.clone());
            }
        }
        out
    };
    let paths = Paths::detect();

    if let Some(l) = lang_override {
        crate::i18n::set(l);
    } else {
        crate::i18n::init_from_settings(&load_settings(&paths));
    }

    let Some(cmd) = args.first().cloned() else {
        return (err(&crate::i18n::tr("cli.missing_cmd")), 2);
    };
    let rest = &args[1..];

    let out = match cmd.as_str() {
        "state" => match get_state(&paths) {
            Ok(st) => ok(serde_json::to_value(st).unwrap_or(Value::Null)),
            Err(e) => return (err(&e), 1),
        },
        "list" => {
            let accounts = list_accounts(&paths);
            match accounts {
                Ok(a) => ok(json!({
                    "accounts": a.iter().map(|x| json!({
                        "id": x.id, "name": x.name, "created_at": x.created_at,
                        "updated_at": x.updated_at, "hash": x.hash,
                    })).collect::<Vec<_>>()
                })),
                Err(e) => return (err(&e), 1),
            }
        }
        "capture" => {
            let name = flag(rest, "--name");
            match capture_current(&paths, name) {
                Ok(a) => ok(json!({ "id": a.id, "name": a.name })),
                Err(e) => return (err(&e), 1),
            }
        }
        "rename" => {
            let (Some(id), Some(name)) = (flag(rest, "--id"), flag(rest, "--name")) else {
                return (err(&crate::i18n::tr("cli.usage.rename")), 2);
            };
            match rename_account(&paths, &id, &name) {
                Ok(a) => ok(json!({ "id": a.id, "name": a.name })),
                Err(e) => return (err(&e), 1),
            }
        }
        "delete" => {
            let Some(id) = flag(rest, "--id") else {
                return (err(&crate::i18n::tr("cli.usage.delete")), 2);
            };
            match delete_account(&paths, &id) {
                Ok(()) => ok(json!({ "deleted": id })),
                Err(e) => return (err(&e), 1),
            }
        }
        "update" => {
            let Some(id) = flag(rest, "--id") else {
                return (err(&crate::i18n::tr("cli.usage.update")), 2);
            };
            match update_account_from_live(&paths, &id) {
                Ok(a) => ok(json!({ "id": a.id, "name": a.name, "hash": a.hash })),
                Err(e) => return (err(&e), 1),
            }
        }
        "switch" => {
            let Some(id) = flag(rest, "--id") else {
                return (err(&crate::i18n::tr("cli.usage.switch")), 2);
            };
            let force = has_flag(rest, "--force");
            let settings = load_settings(&paths);
            let restart = if has_flag(rest, "--restart") {
                true
            } else if has_flag(rest, "--no-restart") {
                false
            } else {
                settings.launch_after_switch()
            };
            let hot = if let Some(v) = flag(rest, "--hot") {
                parse_bool(&v)
            } else if has_flag(rest, "--no-hot") {
                false
            } else {
                settings.hot_switch()
            };
            match switch_to(&paths, &id, force, restart, hot) {
                Ok(r) => {
                    if let Some(p) = &r.probe {
                        let msg = crate::probe::probe_message(p);
                        if !msg.is_empty() {
                            eprintln!("{msg}");
                        }
                    }
                    ok(serde_json::to_value(&r).unwrap_or(Value::Null))
                }
                Err(e) => return (err(&e), 1),
            }
        }
        "kill" => match kill_zcode() {
            Ok(true) => ok(json!({ "killed": true })),
            Ok(false) => return (err(&crate::i18n::tr("err.zcode.kill_timeout")), 1),
            Err(e) => return (err(&e), 1),
        },
        "quota" => {
            let res = match flag(rest, "--id") {
                Some(id) => account_quota(&paths, &id),
                None => live_quota(&paths),
            };
            match res {
                Ok(q) => ok(serde_json::to_value(&q).unwrap_or(Value::Null)),
                Err(e) => return (err(&e), 1),
            }
        }
        "claim-preview" => {
            let only = flag(rest, "--id");
            let accounts = match list_accounts(&paths) {
                Ok(a) => a,
                Err(e) => return (err(&e), 1),
            };
            let mut any = false;
            let mut items = vec![];
            for acc in accounts.iter().filter(|a| only.as_deref().map_or(true, |id| id == a.id)) {
                let res = ensure_virtual_device_mid(&paths, &acc.id).and_then(|mid| {
                    crate::claim::preview_plans(&paths.home, &acc.credentials, acc.config.as_ref(), Some(mid))
                });
                match res {
                    Ok(plans) => {
                        if !plans.is_empty() {
                            any = true;
                        }
                        items.push(json!({
                            "id": acc.id,
                            "name": acc.name,
                            "plans": plans,
                        }));
                    }
                    Err(e) => items.push(json!({
                        "id": acc.id,
                        "name": acc.name,
                        "error": e,
                    })),
                }
            }
            ok(json!({ "anyClaimable": any, "accounts": items }))
        }
        "behavior" => {
            let las = flag(rest, "--launch-after-switch");
            let ctt = flag(rest, "--close-to-tray");
            let mut s = load_settings(&paths);
            if let Some(v) = las {
                s.launch_after_switch = Some(parse_bool(&v));
            }
            if let Some(v) = ctt {
                s.close_to_tray = Some(parse_bool(&v));
            }
            match save_settings(&paths, &s) {
                Ok(()) => ok(json!({
                    "launch_after_switch": s.launch_after_switch(),
                    "close_to_tray": s.close_to_tray(),
                })),
                Err(e) => return (err(&e), 1),
            }
        }
        "export" => {
            let (Some(id), Some(out_path)) = (flag(rest, "--id"), flag(rest, "--out")) else {
                return (err(&crate::i18n::tr("cli.usage.export")), 2);
            };
            let Some(password) = resolve_password(rest) else {
                return (err(&crate::i18n::tr("cli.pw_hint")), 2);
            };
            match load_account(&paths, &id) {
                Ok(a) => {
                    let payload = export_bundle_value(std::slice::from_ref(&a));
                    match crate::cipher::seal(&payload, &password, crate::cipher::FORMAT_BUNDLE) {
                        Ok(sealed) => match serde_json::to_string_pretty(&sealed) {
                            Ok(body) => match atomic_write(std::path::Path::new(&out_path), &(body + "\n")) {
                                Ok(()) => ok(json!({ "out": out_path, "encrypted": true })),
                                Err(e) => return (err(&crate::i18n::trf("err.write", &[("e", &e)])), 1),
                            },
                            Err(e) => return (err(&crate::i18n::trf("err.serialize", &[("e", &e.to_string())])), 1),
                        },
                        Err(e) => return (err(&e), 1),
                    }
                }
                Err(e) => return (err(&e), 1),
            }
        }
        "export-all" => {
            let Some(out_path) = flag(rest, "--out") else {
                return (err(&crate::i18n::tr("cli.usage.export_all")), 2);
            };
            let Some(password) = resolve_password(rest) else {
                return (err(&crate::i18n::tr("cli.pw_hint")), 2);
            };
            match list_accounts(&paths).map(|a| export_bundle_value(&a)) {
                Ok(payload) => match crate::cipher::seal(&payload, &password, crate::cipher::FORMAT_BUNDLE) {
                    Ok(sealed) => match atomic_write(std::path::Path::new(&out_path), &(serde_json::to_string_pretty(&sealed).unwrap() + "\n")) {
                        Ok(()) => ok(json!({ "out": out_path, "encrypted": true })),
                        Err(e) => return (err(&crate::i18n::trf("err.write", &[("e", &e)])), 1),
                    },
                    Err(e) => return (err(&e), 1),
                },
                Err(e) => return (err(&e), 1),
            }
        }
        "import" => {
            let Some(file) = flag(rest, "--file") else {
                return (err(&crate::i18n::tr("cli.usage.import")), 2);
            };
            let raw = match fs::read_to_string(&file) {
                Ok(r) => r,
                Err(e) => return (err(&crate::i18n::trf("cli.read_fail", &[("e", &e.to_string())])), 1),
            };
            let v: Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(e) => return (err(&crate::i18n::trf("cli.json_fail", &[("e", &e.to_string())])), 1),
            };
            if !crate::cipher::is_sealed(&v) {
                return (err(&crate::i18n::tr("err.import.not_sealed_plain")), 2);
            }
            if v.get("format").and_then(|f| f.as_str()) != Some(crate::cipher::FORMAT_BUNDLE) {
                return (err(&crate::i18n::tr("err.import.not_bundle_plain")), 2);
            }
            let Some(password) = resolve_password(rest) else {
                return (err(&crate::i18n::tr("cli.pw_hint")), 2);
            };
            let entries = match crate::cipher::open(&v, &password) {
                Ok(payload) => vec![(file, payload)],
                Err(e) => return (err(&e), 1),
            };
            match import_values(&paths, &entries) {
                Ok(rep) => ok(serde_json::to_value(&rep).unwrap_or(Value::Null)),
                Err(e) => return (err(&e), 1),
            }
        }
        "setpath" => {
            let Some(p) = flag(rest, "--path") else {
                return (err(&crate::i18n::tr("cli.usage.setpath")), 2);
            };
            let mut s = load_settings(&paths);
            s.zcode_path = Some(p);
            match save_settings(&paths, &s) {
                Ok(()) => ok(json!({})),
                Err(e) => return (err(&e), 1),
            }
        }
        "launch" => {
            let (p, ok_path) = effective_zcode_path(&paths);
            if !ok_path {
                return (err(&crate::i18n::trf("err.zcode.path_invalid", &[("p", &p)])), 1);
            }
            match launch_zcode(&p) {
                Ok(()) => ok(json!({ "launched": p })),
                Err(e) => return (err(&e), 1),
            }
        }
        other => return (err(&crate::i18n::trf("cli.unknown_cmd", &[("cmd", other)])), 2),
    };
    (out, 0)
}
