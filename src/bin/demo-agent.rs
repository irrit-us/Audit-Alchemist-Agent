//! Deterministic plumbing fixture, deliberately not a vulnerability discovery model.
use audit_harness::{
    context::snapshot,
    protocol::{Finding, Request, Response, Severity, VERSION},
};
use std::io::Read;

fn main() -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().take(131_073).read_to_string(&mut input)?;
    anyhow::ensure!(input.len() <= 131_072, "oversized request");
    let request: Request = serde_json::from_str(&input)?;
    let sources = snapshot(&std::env::current_dir()?, &request.target, 262_144)?;
    let mut findings = vec![];
    for source in sources {
        for (index, line) in source.content.lines().enumerate() {
            let cwe = if line.contains("shell=True") {
                Some("CWE-78")
            } else if line.contains("connection.execute(query)") {
                Some("CWE-89")
            } else if line.contains("return eval(") {
                Some("CWE-95")
            } else {
                None
            };
            if let Some(cwe) = cwe {
                findings.push(Finding {
                    cwe: cwe.into(),
                    path: source.path.clone(),
                    line: index as u32 + 1,
                    severity: Severity::High,
                    title: "Synthetic fixture pattern".into(),
                    evidence: "Deterministic source pattern match for harness validation only."
                        .into(),
                });
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string(&Response {
            schema_version: VERSION,
            findings
        })?
    );
    Ok(())
}
