//! Repository skill, manifest, frontmatter, and script validator (Rust port of `tests/check.mjs`).

use anyhow::{bail, Result};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Strict frontmatter parser checking unique keys, valid `name` and `description` strings,
/// and rejecting unquoted colons in values that break strict YAML parsers in Claude Code / Codex / Cursor / Gemini CLI.
fn parse_skill_frontmatter(fm_text: &str, where_label: &str, errors: &mut Vec<String>) -> Option<(String, String)> {
    let mut seen_keys = HashSet::new();
    let mut name = None;
    let mut description = None;

    for raw_line in fm_text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            errors.push(format!("{}: YAML syntax error on line {:?}", where_label, line));
            return None;
        };
        let key = k.trim().to_string();
        if !seen_keys.insert(key.clone()) {
            errors.push(format!("{}: duplicate YAML key {:?}", where_label, key));
            return None;
        }
        let mut val = v.trim().to_string();
        let is_quoted = (val.starts_with('"') && val.ends_with('"'))
            || (val.starts_with('\'') && val.ends_with('\''));
        if !is_quoted && val.contains(": ") {
            errors.push(format!(
                "{}: unquoted ': ' in YAML value for key {:?}",
                where_label, key
            ));
            return None;
        }
        if is_quoted && val.len() >= 2 {
            val = val[1..val.len() - 1].to_string();
        }
        match key.as_str() {
            "name" => name = Some(val),
            "description" => description = Some(val),
            _ => {}
        }
    }

    Some((name.unwrap_or_default(), description.unwrap_or_default()))
}

fn check_openai_yaml(yaml_text: &str, where_label: &str, errors: &mut Vec<String>) {
    let has_interface = yaml_text.lines().any(|l| l.trim().starts_with("interface:"));
    let has_display_name = yaml_text
        .lines()
        .any(|l| l.trim().starts_with("display_name:") && !l.split_once(':').unwrap().1.trim().is_empty());
    if !has_interface || !has_display_name {
        errors.push(format!("{}: interface.display_name missing", where_label));
    }
}

fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut sorted: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        sorted.sort_by_key(|e| e.file_name());
        for entry in sorted {
            let p = entry.path();
            let name = entry.file_name();
            if name == "node_modules" || name == "target" || name == ".git" {
                continue;
            }
            if p.is_dir() {
                out.extend(walk_files(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// Validates all skills, plugin manifests, file references, and scripts in `root`.
pub fn verify_repo(root: &Path) -> Result<(usize, usize, usize)> {
    let skills_dir = root.join("skills");
    let mut errors: Vec<String> = Vec::new();
    let mut names: HashMap<String, String> = HashMap::new();

    let kebab_re = Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$")?;
    let ref_re = Regex::new(r"`((?:scripts|assets|references)/[^`\s]+?)`")?;

    let mut skill_dirs: Vec<_> = std::fs::read_dir(&skills_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    skill_dirs.sort_by_key(|e| e.file_name());

    for entry in skill_dirs {
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let skill_md = entry.path().join("SKILL.md");
        let where_label = format!("skills/{}/SKILL.md", dir_name);
        if !skill_md.exists() {
            errors.push(format!("skills/{}: no SKILL.md", dir_name));
            continue;
        }
        let src = std::fs::read_to_string(&skill_md)?;
        let normalized = src.replace("\r\n", "\n");
        let Some(rest) = normalized.strip_prefix("---\n") else {
            errors.push(format!("{}: no frontmatter block at the top", where_label));
            continue;
        };
        let Some((fm_block, _body)) = rest.split_once("\n---\n") else {
            errors.push(format!("{}: unclosed frontmatter block", where_label));
            continue;
        };

        let Some((name, description)) = parse_skill_frontmatter(fm_block, &where_label, &mut errors)
        else {
            continue;
        };

        if name.is_empty() || !kebab_re.is_match(&name) || name.len() > 64 {
            errors.push(format!(
                "{}: name must be kebab-case, 64 characters at most (got {:?})",
                where_label, name
            ));
        }
        if name != dir_name {
            errors.push(format!(
                "{}: name {:?} should match its folder {:?}",
                where_label, name, dir_name
            ));
        }
        if description.trim().is_empty() {
            errors.push(format!("{}: description must be a non-empty string", where_label));
        } else if description.len() > 1024 {
            errors.push(format!(
                "{}: description is {} characters; agents allow 1024 at most",
                where_label,
                description.len()
            ));
        }
        if let Some(prev) = names.get(&name) {
            errors.push(format!(
                "{}: name {:?} is also used by {}",
                where_label, name, prev
            ));
        } else {
            names.insert(name, where_label.clone());
        }
        if src.contains('—') {
            errors.push(format!("{}: contains an em dash", where_label));
        }

        for caps in ref_re.captures_iter(&src) {
            let rel_ref = &caps[1];
            if !entry.path().join(rel_ref).exists() {
                errors.push(format!(
                    "{}: mentions {}, which does not exist",
                    where_label, rel_ref
                ));
            }
        }

        let openai_path = entry.path().join("agents").join("openai.yaml");
        if openai_path.exists() {
            let o_src = std::fs::read_to_string(&openai_path)?;
            check_openai_yaml(
                &o_src,
                &format!("skills/{}/agents/openai.yaml", dir_name),
                &mut errors,
            );
        }
    }

    let manifests = [
        ".claude-plugin/marketplace.json",
        ".codex-plugin/plugin.json",
        ".agents/plugins/marketplace.json",
        ".cursor-plugin/plugin.json",
        "gemini-extension.json",
    ];
    let mut versions = HashSet::new();
    for rel in manifests {
        let p = root.join(rel);
        if !p.exists() {
            errors.push(format!("{}: missing", rel));
            continue;
        }
        let content = std::fs::read_to_string(&p)?;
        let j: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                errors.push(format!("{}: invalid JSON: {}", rel, e));
                continue;
            }
        };
        if j.get("name").and_then(|v| v.as_str()) != Some("open-3d-skills") {
            errors.push(format!(
                "{}: name is {:?}, expected \"open-3d-skills\"",
                rel,
                j.get("name")
            ));
        }
        if let Some(plugins) = j.get("plugins").and_then(|v| v.as_array()) {
            for plugin in plugins {
                if let Some(ver) = plugin.get("version").and_then(|v| v.as_str()) {
                    versions.insert(ver.to_string());
                }
                if let Some(skills_arr) = plugin.get("skills").and_then(|v| v.as_array()) {
                    for s in skills_arr.iter().filter_map(|x| x.as_str()) {
                        if !root.join(s).join("SKILL.md").exists() {
                            errors.push(format!("{}: lists {}, which has no SKILL.md", rel, s));
                        }
                    }
                }
            }
        }
        if let Some(ver) = j.get("version").and_then(|v| v.as_str()) {
            versions.insert(ver.to_string());
        }
        if let Some(skills_str) = j.get("skills").and_then(|v| v.as_str()) {
            if !root.join(skills_str).exists() {
                errors.push(format!("{}: skills path {} does not exist", rel, skills_str));
            }
        }
        if let Some(logo) = j.get("logo").and_then(|v| v.as_str()) {
            if !root.join(logo).exists() {
                errors.push(format!("{}: logo {} does not exist", rel, logo));
            }
        }
    }
    if versions.len() > 1 {
        let mut v_list: Vec<_> = versions.into_iter().collect();
        v_list.sort();
        errors.push(format!("manifests: versions disagree: {}", v_list.join(", ")));
    }

    let claude_manifest_path = root.join(".claude-plugin/marketplace.json");
    if claude_manifest_path.exists() {
        if let Ok(j) =
            serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&claude_manifest_path)?)
        {
            if let Some(plugins) = j.get("plugins").and_then(|v| v.as_array()) {
                for plugin in plugins {
                    if plugin.get("source").and_then(|v| v.as_str()) == Some("./")
                        && plugin.get("skills").is_some()
                    {
                        errors.push(format!(
                            ".claude-plugin/marketplace.json: plugin {:?} lists skills that skills/ already provides; each would load twice",
                            plugin.get("name").and_then(|v| v.as_str()).unwrap_or("")
                        ));
                    }
                }
            }
        }
    }

    let files = walk_files(&skills_dir);
    let mut script_count = 0usize;
    let has_node = std::process::Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());

    for f in &files {
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
        if matches!(ext, "rs" | "js" | "mjs") {
            script_count += 1;
            let src = std::fs::read_to_string(f)?;
            if src.trim().is_empty() {
                errors.push(format!("{}: empty script file", f.display()));
            }
            if matches!(ext, "js" | "mjs") && has_node {
                let out = std::process::Command::new("node")
                    .arg("--check")
                    .arg(f)
                    .output()?;
                if !out.status.success() {
                    let rel_f = f.strip_prefix(root).unwrap_or(f);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let summary: Vec<&str> = stderr.lines().take(3).collect();
                    errors.push(format!("{}: {}", rel_f.display(), summary.join(" ")));
                }
            }
        }
    }

    if !errors.is_empty() {
        eprintln!("not ok - {} problem(s):", errors.len());
        for e in &errors {
            eprintln!("  {}", e);
        }
        bail!("{} verification problem(s) found", errors.len());
    }

    println!(
        "ok - {} skills load, {} manifests agree, {} scripts verified",
        names.len(),
        manifests.len(),
        script_count
    );
    Ok((names.len(), manifests.len(), script_count))
}
