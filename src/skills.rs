//! Compile-time skill catalog: workspace files cannot replace built-in guidance.
use anyhow::{bail, Result};
use serde_json::{json, Value};

struct Skill {
    body: &'static str,
    resources: &'static [(&'static str, &'static str)],
}

const SKILLS: &[Skill] = &[
    Skill {
        body: include_str!("../skills/debugger-selection/SKILL.md"),
        resources: &[
            (
                "references/native.md",
                include_str!("../skills/debugger-selection/references/native.md"),
            ),
            (
                "references/javascript.md",
                include_str!("../skills/debugger-selection/references/javascript.md"),
            ),
            (
                "references/managed.md",
                include_str!("../skills/debugger-selection/references/managed.md"),
            ),
            (
                "references/dynamic.md",
                include_str!("../skills/debugger-selection/references/dynamic.md"),
            ),
            (
                "references/go-shell.md",
                include_str!("../skills/debugger-selection/references/go-shell.md"),
            ),
        ],
    },
    Skill {
        body: include_str!("../skills/foundry-debugging/SKILL.md"),
        resources: &[
            (
                "references/compatibility.md",
                include_str!("../skills/foundry-debugging/references/compatibility.md"),
            ),
            (
                "scripts/RawDebug.sol",
                include_str!("../skills/foundry-debugging/scripts/RawDebug.sol"),
            ),
            (
                "scripts/TypedDebug.t.sol",
                include_str!("../skills/foundry-debugging/scripts/TypedDebug.t.sol"),
            ),
            (
                "scripts/forge_trace.py",
                include_str!("../skills/foundry-debugging/scripts/forge_trace.py"),
            ),
        ],
    },
    Skill {
        body: include_str!("../skills/gdb-debugging/SKILL.md"),
        resources: &[(
            "scripts/capture.gdb",
            include_str!("../skills/gdb-debugging/scripts/capture.gdb"),
        )],
    },
    Skill {
        body: include_str!("../skills/node-inspector/SKILL.md"),
        resources: &[(
            "scripts/inspect.mjs",
            include_str!("../skills/node-inspector/scripts/inspect.mjs"),
        )],
    },
    Skill {
        body: include_str!("../skills/pwntools-debugging/SKILL.md"),
        resources: &[(
            "scripts/tube_probe.py",
            include_str!("../skills/pwntools-debugging/scripts/tube_probe.py"),
        )],
    },
    Skill {
        body: include_str!("../skills/code-audit/SKILL.md"),
        resources: &[],
    },
    Skill {
        body: include_str!("../skills/poc-validation/SKILL.md"),
        resources: &[],
    },
    Skill {
        body: include_str!("../skills/native-debugging/SKILL.md"),
        resources: &[(
            "references/pwndbg.md",
            include_str!("../skills/native-debugging/references/pwndbg.md"),
        )],
    },
    Skill {
        body: include_str!("../skills/tmux-debugging/SKILL.md"),
        resources: &[],
    },
    Skill {
        body: include_str!("../skills/finding-review/SKILL.md"),
        resources: &[],
    },
];

fn field(body: &'static str, key: &str) -> &'static str {
    body.lines()
        .skip(1)
        .take_while(|line| *line != "---")
        .find_map(|line| line.strip_prefix(key))
        .expect("built-in skill metadata")
        .trim()
}

pub fn catalog() -> Value {
    Value::Array(
        SKILLS
            .iter()
            .map(|skill| {
                json!({
                    "name": field(skill.body, "name:"),
                    "description": field(skill.body, "description:"),
                })
            })
            .collect(),
    )
}

pub fn load(name: &str, resource: Option<&str>) -> Result<Value> {
    let Some(skill) = SKILLS.iter().find(|s| field(s.body, "name:") == name) else {
        bail!("unknown built-in skill; use a name from the supplied skills catalog");
    };
    let resource = resource.unwrap_or("SKILL.md");
    let content = if resource == "SKILL.md" {
        skill.body
    } else {
        skill
            .resources
            .iter()
            .find(|(path, _)| *path == resource)
            .map(|(_, content)| *content)
            .ok_or_else(|| {
                anyhow::anyhow!("unknown skill resource; use an exact available_resources path")
            })?
    };
    Ok(json!({"name":name,"resource":resource,"content":content,
        "available_resources":skill.resources.iter().map(|(path, _)| path).collect::<Vec<_>>()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_and_resources_are_bounded_and_closed() {
        let mut names = std::collections::BTreeSet::new();
        for skill in SKILLS {
            let name = field(skill.body, "name:");
            assert!(names.insert(name));
            assert!(!field(skill.body, "description:").is_empty());
            assert!(skill.body.len() < 8192);
            assert_eq!(load(name, None).unwrap()["content"], skill.body);
            for (path, content) in skill.resources {
                assert!(content.len() < 8192);
                assert_eq!(load(name, Some(path)).unwrap()["content"], *content);
            }
            assert!(load(name, Some("../../outside.md")).is_err());
        }
        assert!(load("unknown", None).is_err());
    }
}
