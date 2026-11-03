//! Gated runbook engine with dry-run plans.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Each runbook step declares gates (a minimum health score and a
//! maximum open-alert count). `plan` renders a dry-run per step;
//! `execute` applies every step only when all gates pass and aborts at the
//! first failing gate with a concrete reason.

use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_RUNBOOKS: usize = 1_000;
pub const MAX_STEPS: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RunbookError {
    #[error("runbook name must not be empty")]
    EmptyName,
    #[error("runbook registry is full (max {MAX_RUNBOOKS} runbooks)")]
    RegistryFull,
    #[error("runbook {0} must have between 1 and {MAX_STEPS} steps")]
    InvalidStepCount(String),
    #[error("runbook {0} has a duplicate step {1}")]
    DuplicateStep(String, String),
    #[error("unknown runbook {0}")]
    UnknownRunbook(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    pub min_health: u8,
    pub max_open_alerts: u32,
    pub action: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runbook {
    pub name: String,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepStatus {
    Ready,
    Skipped { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedStep {
    pub step: String,
    pub action: String,
    pub status: StepStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Execution {
    Applied { actions: Vec<String> },
    Aborted { step: String, reason: String },
}

#[derive(Debug)]
pub struct Runbooks {
    max_runbooks: usize,
    runbooks: BTreeMap<String, Runbook>,
}

impl Runbooks {
    pub fn new(max_runbooks: usize) -> Result<Self, RunbookError> {
        if max_runbooks == 0 || max_runbooks > MAX_RUNBOOKS {
            return Err(RunbookError::RegistryFull);
        }
        Ok(Self {
            max_runbooks,
            runbooks: BTreeMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.runbooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.runbooks.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Runbook> {
        self.runbooks.get(name)
    }

    /// Registers a runbook after validating its name and step list.
    pub fn register(&mut self, runbook: Runbook) -> Result<(), RunbookError> {
        if runbook.name.is_empty() {
            return Err(RunbookError::EmptyName);
        }
        if self.runbooks.len() >= self.max_runbooks {
            return Err(RunbookError::RegistryFull);
        }
        if runbook.steps.is_empty() || runbook.steps.len() > MAX_STEPS {
            return Err(RunbookError::InvalidStepCount(runbook.name.clone()));
        }
        let mut seen: Vec<&str> = Vec::new();
        for step in &runbook.steps {
            if seen.contains(&step.name.as_str()) {
                return Err(RunbookError::DuplicateStep(
                    runbook.name.clone(),
                    step.name.clone(),
                ));
            }
            seen.push(step.name.as_str());
        }
        self.runbooks.insert(runbook.name.clone(), runbook);
        Ok(())
    }

    /// Renders a dry-run plan: every step is Ready or Skipped with a reason.
    pub fn plan(
        &self,
        name: &str,
        health: u8,
        open_alerts: u32,
    ) -> Result<Vec<PlannedStep>, RunbookError> {
        let runbook = self
            .runbooks
            .get(name)
            .ok_or_else(|| RunbookError::UnknownRunbook(name.to_string()))?;
        Ok(runbook
            .steps
            .iter()
            .map(|step| PlannedStep {
                step: step.name.clone(),
                action: step.action.clone(),
                status: gate_status(step, health, open_alerts),
            })
            .collect())
    }

    /// Executes a runbook: applies every action only when all gates pass,
    /// and aborts at the first failing gate with a concrete reason.
    pub fn execute(
        &self,
        name: &str,
        health: u8,
        open_alerts: u32,
    ) -> Result<Execution, RunbookError> {
        let runbook = self
            .runbooks
            .get(name)
            .ok_or_else(|| RunbookError::UnknownRunbook(name.to_string()))?;
        let mut actions = Vec::new();
        for step in &runbook.steps {
            match gate_status(step, health, open_alerts) {
                StepStatus::Ready => actions.push(step.action.clone()),
                StepStatus::Skipped { reason } => {
                    return Ok(Execution::Aborted {
                        step: step.name.clone(),
                        reason,
                    })
                }
            }
        }
        Ok(Execution::Applied { actions })
    }
}

fn gate_status(step: &Step, health: u8, open_alerts: u32) -> StepStatus {
    if health < step.min_health {
        return StepStatus::Skipped {
            reason: format!(
                "health {health} is below the required minimum {}",
                step.min_health
            ),
        };
    }
    if open_alerts > step.max_open_alerts {
        return StepStatus::Skipped {
            reason: format!(
                "{open_alerts} open alerts exceed the allowed maximum of {}",
                step.max_open_alerts
            ),
        };
    }
    StepStatus::Ready
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(name: &str, min_health: u8, max_open_alerts: u32) -> Step {
        Step {
            name: name.to_string(),
            min_health,
            max_open_alerts,
            action: format!("do-{name}"),
        }
    }

    fn sample() -> Runbook {
        Runbook {
            name: "drain-and-repair".into(),
            steps: vec![step("drain", 80, 5), step("repair", 60, 2)],
        }
    }

    #[test]
    fn validates_registration() {
        let mut runbooks = Runbooks::new(1).unwrap();
        assert_eq!(
            runbooks
                .register(Runbook {
                    name: String::new(),
                    steps: vec![step("a", 0, 0)],
                })
                .unwrap_err(),
            RunbookError::EmptyName
        );
        assert_eq!(
            runbooks
                .register(Runbook {
                    name: "empty".into(),
                    steps: Vec::new(),
                })
                .unwrap_err(),
            RunbookError::InvalidStepCount("empty".into())
        );
        assert_eq!(
            runbooks
                .register(Runbook {
                    name: "dup".into(),
                    steps: vec![step("a", 0, 0), step("a", 0, 0)],
                })
                .unwrap_err(),
            RunbookError::DuplicateStep("dup".into(), "a".into())
        );
        runbooks.register(sample()).unwrap();
        assert_eq!(
            runbooks.register(sample()).unwrap_err(),
            RunbookError::RegistryFull
        );
    }

    #[test]
    fn unknown_runbook_is_typed_error() {
        let runbooks = Runbooks::new(4).unwrap();
        assert_eq!(
            runbooks.plan("ghost", 100, 0).unwrap_err(),
            RunbookError::UnknownRunbook("ghost".into())
        );
    }

    #[test]
    fn plan_marks_gates_and_reasons() {
        let mut runbooks = Runbooks::new(4).unwrap();
        runbooks.register(sample()).unwrap();
        let plan = runbooks.plan("drain-and-repair", 90, 0).unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].status, StepStatus::Ready);
        assert_eq!(plan[1].status, StepStatus::Ready);

        let plan = runbooks.plan("drain-and-repair", 70, 0).unwrap();
        assert_eq!(
            plan[0].status,
            StepStatus::Skipped {
                reason: "health 70 is below the required minimum 80".into()
            }
        );
        assert_eq!(plan[1].status, StepStatus::Ready);

        let plan = runbooks.plan("drain-and-repair", 100, 3).unwrap();
        assert_eq!(plan[0].status, StepStatus::Ready);
        assert_eq!(
            plan[1].status,
            StepStatus::Skipped {
                reason: "3 open alerts exceed the allowed maximum of 2".into()
            }
        );
    }

    #[test]
    fn execute_applies_all_or_aborts_at_first_gate() {
        let mut runbooks = Runbooks::new(4).unwrap();
        runbooks.register(sample()).unwrap();
        assert_eq!(
            runbooks.execute("drain-and-repair", 95, 0).unwrap(),
            Execution::Applied {
                actions: vec!["do-drain".into(), "do-repair".into()]
            }
        );
        assert_eq!(
            runbooks.execute("drain-and-repair", 50, 0).unwrap(),
            Execution::Aborted {
                step: "drain".into(),
                reason: "health 50 is below the required minimum 80".into()
            }
        );
    }
}
