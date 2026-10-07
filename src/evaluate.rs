use crate::{
    dataset::Case,
    protocol::FindingKey,
    runner::{Outcome, RunResult},
};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Default, Serialize)]
pub struct Metrics {
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub f1: Option<f64>,
    pub successful_cases: usize,
    pub failed_cases: usize,
    pub exact_match_cases: usize,
    pub elapsed_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct CaseReport {
    pub run: RunResult,
    pub matched: Vec<FindingKey>,
    pub unexpected: Vec<FindingKey>,
    pub missed: Vec<FindingKey>,
    pub duplicate_findings: usize,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub dataset: String,
    pub executable: String,
    pub args: Vec<String>,
    pub jobs: usize,
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
    pub metrics: Metrics,
    pub cases: Vec<CaseReport>,
}

pub fn score(case: &Case, run: RunResult) -> CaseReport {
    let expected: BTreeSet<_> = case.expected.iter().cloned().collect();
    let actual: BTreeSet<_> = run.findings.iter().map(|f| f.key()).collect();
    CaseReport {
        matched: actual.intersection(&expected).cloned().collect(),
        unexpected: actual.difference(&expected).cloned().collect(),
        missed: expected.difference(&actual).cloned().collect(),
        duplicate_findings: run.findings.len() - actual.len(),
        run,
    }
}

pub fn aggregate(cases: &[CaseReport], elapsed_ms: u64) -> Metrics {
    let mut m = Metrics {
        elapsed_ms,
        ..Metrics::default()
    };
    for case in cases {
        m.true_positives += case.matched.len();
        m.false_positives += case.unexpected.len();
        m.false_negatives += case.missed.len();
        if case.run.outcome == Outcome::Success {
            m.successful_cases += 1;
            if case.unexpected.is_empty() && case.missed.is_empty() {
                m.exact_match_cases += 1;
            }
        } else {
            m.failed_cases += 1;
        }
    }
    m.precision = ratio(m.true_positives, m.true_positives + m.false_positives);
    m.recall = ratio(m.true_positives, m.true_positives + m.false_negatives);
    m.f1 = ratio(
        2 * m.true_positives,
        2 * m.true_positives + m.false_positives + m.false_negatives,
    );
    m
}

fn ratio(n: usize, d: usize) -> Option<f64> {
    (d != 0).then(|| n as f64 / d as f64)
}
