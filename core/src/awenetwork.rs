//! AWENETwork — the founder-governed work platform of the ONECOIN economy.
//!
//! Only the founder publishes, modifies and closes official tasks, and only
//! the founder finally reviews submitted work. Rewards are minted strictly
//! after founder approval and exactly once per payout id. Community members
//! may suggest tasks, but a suggestion never becomes an official task until
//! the founder approves and publishes it. Work profiles and levels reflect
//! real approved work only and never mint ONECOIN by themselves.

use crate::founder::{FounderAction, FounderAuthority, FounderError};
use crate::identity::{AweId, Identity};
use crate::onecoin::OnecoinLedger;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const MAX_TASKS: usize = 4_096;
pub const MAX_SUBMISSIONS: usize = 20_000;
pub const MAX_SUGGESTIONS: usize = 4_096;
pub const MAX_TITLE_CHARS: usize = 128;
pub const MAX_DESCRIPTION_CHARS: usize = 4_096;
pub const MAX_OUTCOME_CHARS: usize = 2_048;
pub const MAX_SKILLS: usize = 32;
pub const MAX_PHASES: usize = 16;
pub const MAX_ACCEPTANCE_CRITERIA: usize = 32;
pub const MAX_CRITERION_CHARS: usize = 256;
pub const MAX_EVIDENCE_ITEMS: usize = 16;
pub const MAX_EVIDENCE_CHARS: usize = 512;
pub const MAX_REVIEW_REASON_CHARS: usize = 256;
pub const MAX_SPLITS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum WorkError {
    #[error(transparent)]
    Founder(#[from] FounderError),
    #[error("invalid task specification: {0}")]
    InvalidSpec(&'static str),
    #[error("task registry is full (max {MAX_TASKS} tasks)")]
    TaskRegistryFull,
    #[error("submission registry is full (max {MAX_SUBMISSIONS} submissions)")]
    SubmissionRegistryFull,
    #[error("suggestion registry is full (max {MAX_SUGGESTIONS} suggestions)")]
    SuggestionRegistryFull,
    #[error("unknown task {0}")]
    UnknownTask(u64),
    #[error("unknown submission {0}")]
    UnknownSubmission(u64),
    #[error("unknown suggestion {0}")]
    UnknownSuggestion(u64),
    #[error("task {0} is not open")]
    TaskNotOpen(u64),
    #[error("task {0} is not published")]
    TaskNotPublished(u64),
    #[error("task {0} deadline has passed")]
    DeadlinePassed(u64),
    #[error("task {0} does not allow team submissions")]
    TeamWorkNotAllowed(u64),
    #[error("contributor has not accepted task {0}")]
    NotAssigned(u64),
    #[error("submission {0} is not awaiting review")]
    NotAwaitingReview(u64),
    #[error("submission {0} belongs to another contributor")]
    NotContributor(u64),
    #[error("evidence must contain between 1 and {MAX_EVIDENCE_ITEMS} non-empty items")]
    InvalidEvidence,
    #[error("review reason is required and must not exceed {MAX_REVIEW_REASON_CHARS} characters")]
    InvalidReason,
    #[error(
        "payout split must be non-empty, at most {MAX_SPLITS} parts and sum to the task reward"
    )]
    InvalidSplit,
    #[error("phase rewards must sum to the task reward")]
    PhaseRewardsMismatch,
    #[error("suggestion {0} was already approved")]
    SuggestionAlreadyApproved(u64),
    #[error("ledger rejected the payout: {0}")]
    Ledger(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkCategory {
    Engineering,
    Design,
    Content,
    Network,
    Community,
}

impl WorkCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Engineering => "engineering",
            Self::Design => "design",
            Self::Content => "content",
            Self::Network => "network",
            Self::Community => "community",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Complexity {
    Beginner,
    Intermediate,
    Advanced,
    Expert,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkTaskStatus {
    Draft,
    Published,
    Closed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubmissionStatus {
    Submitted,
    Approved,
    Rejected,
    Revising,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskPhase {
    pub name: String,
    pub acceptance_criteria: String,
    pub reward_atoms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkTaskSpec {
    pub title: String,
    pub description: String,
    pub category: WorkCategory,
    pub complexity: Complexity,
    pub deadline_unix: Option<u64>,
    pub required_skills: BTreeSet<String>,
    pub expected_outcome: String,
    pub acceptance_criteria: Vec<String>,
    pub reward_atoms: u128,
    pub phases: Vec<TaskPhase>,
    pub team_allowed: bool,
    pub submission_format: String,
}

impl WorkTaskSpec {
    /// The reward a full completion of this task pays: the sum of the phase
    /// rewards when phases exist, otherwise the task reward.
    pub fn total_reward_atoms(&self) -> u128 {
        if self.phases.is_empty() {
            self.reward_atoms
        } else {
            self.phases
                .iter()
                .fold(0u128, |acc, phase| acc.saturating_add(phase.reward_atoms))
        }
    }

    pub fn validate(&self) -> Result<(), WorkError> {
        let title = self.title.trim();
        if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
            return Err(WorkError::InvalidSpec("title must be 1-128 characters"));
        }
        if self.description.trim().is_empty()
            || self.description.chars().count() > MAX_DESCRIPTION_CHARS
        {
            return Err(WorkError::InvalidSpec(
                "description must be 1-4096 characters",
            ));
        }
        if self.expected_outcome.trim().is_empty()
            || self.expected_outcome.chars().count() > MAX_OUTCOME_CHARS
        {
            return Err(WorkError::InvalidSpec(
                "expected outcome must be 1-2048 characters",
            ));
        }
        if self.reward_atoms == 0 {
            return Err(WorkError::InvalidSpec("reward must be positive"));
        }
        if self.required_skills.len() > MAX_SKILLS {
            return Err(WorkError::InvalidSpec("too many required skills"));
        }
        if self.acceptance_criteria.is_empty()
            || self.acceptance_criteria.len() > MAX_ACCEPTANCE_CRITERIA
        {
            return Err(WorkError::InvalidSpec("acceptance criteria are required"));
        }
        if self
            .acceptance_criteria
            .iter()
            .any(|c| c.trim().is_empty() || c.chars().count() > MAX_CRITERION_CHARS)
        {
            return Err(WorkError::InvalidSpec(
                "each acceptance criterion must be 1-256 characters",
            ));
        }
        if self.phases.len() > MAX_PHASES {
            return Err(WorkError::InvalidSpec("too many phases"));
        }
        if !self.phases.is_empty() {
            if self.phases.iter().any(|phase| {
                phase.reward_atoms == 0
                    || phase.name.trim().is_empty()
                    || phase.acceptance_criteria.trim().is_empty()
            }) {
                return Err(WorkError::InvalidSpec(
                    "each phase needs a name, criteria and a positive reward",
                ));
            }
            if self.total_reward_atoms() != self.reward_atoms {
                return Err(WorkError::PhaseRewardsMismatch);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkTask {
    pub id: u64,
    pub spec: WorkTaskSpec,
    pub status: WorkTaskStatus,
    pub published_at_unix: Option<u64>,
    pub author: AweId,
}

impl WorkTask {
    pub fn publisher(&self) -> &AweId {
        &self.author
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewDecision {
    pub approved: bool,
    pub reason: String,
    pub reviewed_at_unix: u64,
    pub reviewer: AweId,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskSubmission {
    pub id: u64,
    pub task_id: u64,
    pub contributor: AweId,
    pub evidence: Vec<String>,
    pub note: Option<String>,
    pub submitted_at_unix: u64,
    pub status: SubmissionStatus,
    pub review: Option<ReviewDecision>,
    pub paid_atoms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskSuggestion {
    pub id: u64,
    pub contributor: AweId,
    pub spec: WorkTaskSpec,
    pub suggested_at_unix: u64,
    pub approved: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayoutReceipt {
    pub task_id: u64,
    pub submission_id: u64,
    pub contributor: AweId,
    pub paid_atoms: u128,
    pub reward_id: [u8; 32],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamPayoutReceipt {
    pub task_id: u64,
    pub submission_id: u64,
    pub recipients: Vec<(AweId, u128)>,
    pub paid_atoms: u128,
    pub reward_id: [u8; 32],
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkProfile {
    pub accepted_tasks: Vec<u64>,
    pub submissions: Vec<u64>,
    pub approved: Vec<u64>,
    pub rejected: Vec<u64>,
    pub revising: Vec<u64>,
    pub earned_atoms: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContributorLevel {
    Beginner,
    Contributor,
    Advanced,
    Expert,
    Master,
}

impl ContributorLevel {
    /// Levels reflect real approved work history only; they never mint
    /// ONECOIN by themselves.
    pub fn from_approved(count: usize) -> Self {
        match count {
            0 => Self::Beginner,
            1..=2 => Self::Contributor,
            3..=5 => Self::Advanced,
            6..=9 => Self::Expert,
            _ => Self::Master,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Beginner => "beginner",
            Self::Contributor => "contributor",
            Self::Advanced => "advanced",
            Self::Expert => "expert",
            Self::Master => "master",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskFilter {
    pub category: Option<WorkCategory>,
    pub complexity: Option<Complexity>,
    pub status: Option<WorkTaskStatus>,
    pub min_reward_atoms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkMarket {
    next_task_id: u64,
    next_submission_id: u64,
    next_suggestion_id: u64,
    tasks: BTreeMap<u64, WorkTask>,
    submissions: BTreeMap<u64, TaskSubmission>,
    suggestions: BTreeMap<u64, TaskSuggestion>,
    assignments: BTreeMap<u64, BTreeSet<String>>,
}

fn payout_id(task_id: u64, submission_id: u64, contributor: &AweId) -> [u8; 32] {
    let mut input = Vec::new();
    input.extend_from_slice(&task_id.to_be_bytes());
    input.extend_from_slice(&submission_id.to_be_bytes());
    input.extend_from_slice(contributor.as_bytes());
    *blake3::hash(&input).as_bytes()
}

fn team_payout_id(task_id: u64, submission_id: u64) -> [u8; 32] {
    let mut input = Vec::new();
    input.extend_from_slice(&task_id.to_be_bytes());
    input.extend_from_slice(&submission_id.to_be_bytes());
    input.extend_from_slice(b"awenetwork-team");
    *blake3::hash(&input).as_bytes()
}

fn validate_evidence(evidence: &[String]) -> Result<(), WorkError> {
    if evidence.is_empty() || evidence.len() > MAX_EVIDENCE_ITEMS {
        return Err(WorkError::InvalidEvidence);
    }
    if evidence
        .iter()
        .any(|item| item.trim().is_empty() || item.len() > MAX_EVIDENCE_CHARS)
    {
        return Err(WorkError::InvalidEvidence);
    }
    Ok(())
}

impl WorkMarket {
    pub fn new() -> Self {
        Self {
            next_task_id: 1,
            next_submission_id: 1,
            next_suggestion_id: 1,
            tasks: BTreeMap::new(),
            submissions: BTreeMap::new(),
            suggestions: BTreeMap::new(),
            assignments: BTreeMap::new(),
        }
    }

    pub fn task(&self, id: u64) -> Option<&WorkTask> {
        self.tasks.get(&id)
    }

    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    pub fn submission(&self, id: u64) -> Option<&TaskSubmission> {
        self.submissions.get(&id)
    }

    pub fn submission_count(&self) -> usize {
        self.submissions.len()
    }

    pub fn suggestion(&self, id: u64) -> Option<&TaskSuggestion> {
        self.suggestions.get(&id)
    }

    pub fn suggestion_count(&self) -> usize {
        self.suggestions.len()
    }

    /// All submissions in id order, for review queues and dashboards.
    pub fn submissions(&self) -> impl Iterator<Item = &TaskSubmission> {
        self.submissions.values()
    }

    /// All task suggestions in id order, for the founder review queue.
    pub fn suggestions(&self) -> impl Iterator<Item = &TaskSuggestion> {
        self.suggestions.values()
    }

    pub fn assignment_count(&self, task_id: u64) -> usize {
        self.assignments.get(&task_id).map_or(0, |set| set.len())
    }

    pub fn find_tasks(&self, filter: &TaskFilter) -> Vec<&WorkTask> {
        self.tasks
            .values()
            .filter(|task| {
                filter
                    .category
                    .is_none_or(|category| task.spec.category == category)
                    && filter
                        .complexity
                        .is_none_or(|complexity| task.spec.complexity == complexity)
                    && filter.status.is_none_or(|status| task.status == status)
                    && task.spec.total_reward_atoms() >= filter.min_reward_atoms
            })
            .collect()
    }

    /// Publishes an official task. Only the founder can do this.
    pub fn publish_task(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        spec: WorkTaskSpec,
        now_unix: u64,
    ) -> Result<u64, WorkError> {
        spec.validate()?;
        if self.tasks.len() >= MAX_TASKS {
            return Err(WorkError::TaskRegistryFull);
        }
        authority.authorize(
            actor_public_key,
            FounderAction::PublishTask,
            now_unix,
            "publish official task",
        )?;
        let id = self.next_task_id;
        self.next_task_id += 1;
        self.tasks.insert(
            id,
            WorkTask {
                id,
                spec,
                status: WorkTaskStatus::Published,
                published_at_unix: Some(now_unix),
                author: authority.founder_awe_id().clone(),
            },
        );
        Ok(id)
    }

    /// Creates a founder draft that becomes visible only after publish_draft.
    pub fn create_draft_task(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        spec: WorkTaskSpec,
        now_unix: u64,
    ) -> Result<u64, WorkError> {
        spec.validate()?;
        if self.tasks.len() >= MAX_TASKS {
            return Err(WorkError::TaskRegistryFull);
        }
        authority.authorize(
            actor_public_key,
            FounderAction::PublishTask,
            now_unix,
            "draft official task",
        )?;
        let id = self.next_task_id;
        self.next_task_id += 1;
        self.tasks.insert(
            id,
            WorkTask {
                id,
                spec,
                status: WorkTaskStatus::Draft,
                published_at_unix: None,
                author: authority.founder_awe_id().clone(),
            },
        );
        Ok(id)
    }

    pub fn publish_draft(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        task_id: u64,
        now_unix: u64,
    ) -> Result<(), WorkError> {
        authority.authorize(
            actor_public_key,
            FounderAction::PublishTask,
            now_unix,
            "publish drafted task",
        )?;
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkError::UnknownTask(task_id))?;
        if task.status != WorkTaskStatus::Draft {
            return Err(WorkError::TaskNotOpen(task_id));
        }
        task.status = WorkTaskStatus::Published;
        task.published_at_unix = Some(now_unix);
        Ok(())
    }

    pub fn modify_task(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        task_id: u64,
        spec: WorkTaskSpec,
        now_unix: u64,
    ) -> Result<(), WorkError> {
        spec.validate()?;
        authority.authorize(
            actor_public_key,
            FounderAction::ModifyTask,
            now_unix,
            "modify official task",
        )?;
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkError::UnknownTask(task_id))?;
        if !matches!(
            task.status,
            WorkTaskStatus::Draft | WorkTaskStatus::Published
        ) {
            return Err(WorkError::TaskNotOpen(task_id));
        }
        task.spec = spec;
        Ok(())
    }

    pub fn close_task(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        task_id: u64,
        now_unix: u64,
    ) -> Result<(), WorkError> {
        authority.authorize(
            actor_public_key,
            FounderAction::CloseTask,
            now_unix,
            "close official task",
        )?;
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkError::UnknownTask(task_id))?;
        if !matches!(
            task.status,
            WorkTaskStatus::Draft | WorkTaskStatus::Published
        ) {
            return Err(WorkError::TaskNotOpen(task_id));
        }
        task.status = WorkTaskStatus::Closed;
        Ok(())
    }

    pub fn cancel_task(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        task_id: u64,
        now_unix: u64,
    ) -> Result<(), WorkError> {
        authority.authorize(
            actor_public_key,
            FounderAction::CloseTask,
            now_unix,
            "cancel official task",
        )?;
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkError::UnknownTask(task_id))?;
        if !matches!(
            task.status,
            WorkTaskStatus::Draft | WorkTaskStatus::Published
        ) {
            return Err(WorkError::TaskNotOpen(task_id));
        }
        task.status = WorkTaskStatus::Cancelled;
        Ok(())
    }

    /// Community members can propose tasks, but a suggestion is never an
    /// official task until the founder approves it.
    pub fn suggest_task(
        &mut self,
        contributor: &AweId,
        spec: WorkTaskSpec,
        now_unix: u64,
    ) -> Result<u64, WorkError> {
        spec.validate()?;
        if self.suggestions.len() >= MAX_SUGGESTIONS {
            return Err(WorkError::SuggestionRegistryFull);
        }
        let id = self.next_suggestion_id;
        self.next_suggestion_id += 1;
        self.suggestions.insert(
            id,
            TaskSuggestion {
                id,
                contributor: contributor.clone(),
                spec,
                suggested_at_unix: now_unix,
                approved: false,
            },
        );
        Ok(id)
    }

    /// The founder turns a suggestion into an official published task.
    pub fn approve_suggestion(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        suggestion_id: u64,
        now_unix: u64,
    ) -> Result<u64, WorkError> {
        if self.tasks.len() >= MAX_TASKS {
            return Err(WorkError::TaskRegistryFull);
        }
        authority.authorize(
            actor_public_key,
            FounderAction::PublishTask,
            now_unix,
            "approve and publish suggested task",
        )?;
        let suggestion = self
            .suggestions
            .get_mut(&suggestion_id)
            .ok_or(WorkError::UnknownSuggestion(suggestion_id))?;
        if suggestion.approved {
            return Err(WorkError::SuggestionAlreadyApproved(suggestion_id));
        }
        suggestion.approved = true;
        let spec = suggestion.spec.clone();
        let id = self.next_task_id;
        self.next_task_id += 1;
        self.tasks.insert(
            id,
            WorkTask {
                id,
                spec,
                status: WorkTaskStatus::Published,
                published_at_unix: Some(now_unix),
                author: authority.founder_awe_id().clone(),
            },
        );
        Ok(id)
    }

    /// A participant accepts a published task before working on it.
    pub fn accept_task(
        &mut self,
        contributor: &AweId,
        task_id: u64,
        now_unix: u64,
    ) -> Result<(), WorkError> {
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkError::UnknownTask(task_id))?;
        if task.status != WorkTaskStatus::Published {
            return Err(WorkError::TaskNotPublished(task_id));
        }
        if let Some(deadline) = task.spec.deadline_unix {
            if now_unix > deadline {
                return Err(WorkError::DeadlinePassed(task_id));
            }
        }
        self.assignments
            .entry(task_id)
            .or_default()
            .insert(contributor.to_hex());
        Ok(())
    }

    /// A participant submits the result of their work for founder review.
    pub fn submit_work(
        &mut self,
        contributor: &AweId,
        task_id: u64,
        evidence: Vec<String>,
        note: Option<String>,
        now_unix: u64,
    ) -> Result<u64, WorkError> {
        validate_evidence(&evidence)?;
        if let Some(note) = &note {
            if note.len() > MAX_REVIEW_REASON_CHARS {
                return Err(WorkError::InvalidReason);
            }
        }
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkError::UnknownTask(task_id))?;
        if task.status != WorkTaskStatus::Published {
            return Err(WorkError::TaskNotPublished(task_id));
        }
        if let Some(deadline) = task.spec.deadline_unix {
            if now_unix > deadline {
                return Err(WorkError::DeadlinePassed(task_id));
            }
        }
        let assigned = self
            .assignments
            .get(&task_id)
            .is_some_and(|set| set.contains(&contributor.to_hex()));
        if !assigned {
            return Err(WorkError::NotAssigned(task_id));
        }
        if self.submissions.len() >= MAX_SUBMISSIONS {
            return Err(WorkError::SubmissionRegistryFull);
        }
        let id = self.next_submission_id;
        self.next_submission_id += 1;
        self.submissions.insert(
            id,
            TaskSubmission {
                id,
                task_id,
                contributor: contributor.clone(),
                evidence,
                note,
                submitted_at_unix: now_unix,
                status: SubmissionStatus::Submitted,
                review: None,
                paid_atoms: 0,
            },
        );
        Ok(id)
    }

    /// The founder approves submitted work; the reward is minted here and
    /// nowhere else. The payout is idempotent per submission.
    pub fn approve_and_pay(
        &mut self,
        authority: &mut FounderAuthority,
        founder: &Identity,
        ledger: &mut OnecoinLedger,
        submission_id: u64,
        now_unix: u64,
    ) -> Result<PayoutReceipt, WorkError> {
        let submission = self
            .submissions
            .get(&submission_id)
            .ok_or(WorkError::UnknownSubmission(submission_id))?;
        if !matches!(
            submission.status,
            SubmissionStatus::Submitted | SubmissionStatus::Revising
        ) {
            return Err(WorkError::NotAwaitingReview(submission_id));
        }
        let task_id = submission.task_id;
        let contributor = submission.contributor.clone();
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkError::UnknownTask(task_id))?;
        let reward = task.spec.total_reward_atoms();
        let reward_id = payout_id(task_id, submission_id, &contributor);
        authority.authorize(
            &founder.public.public_key,
            FounderAction::ReviewSubmission,
            now_unix,
            "approve submission and pay reward",
        )?;
        let paid = ledger
            .mint_approved_reward(&founder.public.public_key, &contributor, reward, reward_id)
            .map_err(WorkError::Ledger)?;
        let submission = self
            .submissions
            .get_mut(&submission_id)
            .expect("submission exists");
        submission.status = SubmissionStatus::Approved;
        submission.review = Some(ReviewDecision {
            approved: true,
            reason: "work approved by founder review".into(),
            reviewed_at_unix: now_unix,
            reviewer: authority.founder_awe_id().clone(),
        });
        submission.paid_atoms = paid;
        Ok(PayoutReceipt {
            task_id,
            submission_id,
            contributor,
            paid_atoms: paid,
            reward_id,
        })
    }

    /// The founder pays a team split that must sum to the task reward.
    pub fn approve_and_pay_team(
        &mut self,
        authority: &mut FounderAuthority,
        founder: &Identity,
        ledger: &mut OnecoinLedger,
        submission_id: u64,
        split: &[(AweId, u128)],
        now_unix: u64,
    ) -> Result<TeamPayoutReceipt, WorkError> {
        if split.is_empty() || split.len() > MAX_SPLITS {
            return Err(WorkError::InvalidSplit);
        }
        let submission = self
            .submissions
            .get(&submission_id)
            .ok_or(WorkError::UnknownSubmission(submission_id))?;
        if !matches!(
            submission.status,
            SubmissionStatus::Submitted | SubmissionStatus::Revising
        ) {
            return Err(WorkError::NotAwaitingReview(submission_id));
        }
        let task_id = submission.task_id;
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkError::UnknownTask(task_id))?;
        if !task.spec.team_allowed {
            return Err(WorkError::TeamWorkNotAllowed(task_id));
        }
        let total = task.spec.total_reward_atoms();
        let split_total = split
            .iter()
            .fold(0u128, |acc, (_, amount)| acc.saturating_add(*amount));
        if split_total != total || split.iter().any(|(_, amount)| *amount == 0) {
            return Err(WorkError::InvalidSplit);
        }
        authority.authorize(
            &founder.public.public_key,
            FounderAction::ReviewSubmission,
            now_unix,
            "approve team submission and pay split",
        )?;
        let root = team_payout_id(task_id, submission_id);
        let mut recipients = Vec::new();
        for (recipient, amount) in split {
            let mut input = Vec::new();
            input.extend_from_slice(&root);
            input.extend_from_slice(recipient.as_bytes());
            let reward_id = *blake3::hash(&input).as_bytes();
            ledger
                .mint_approved_reward(&founder.public.public_key, recipient, *amount, reward_id)
                .map_err(WorkError::Ledger)?;
            recipients.push((recipient.clone(), *amount));
        }
        let submission = self
            .submissions
            .get_mut(&submission_id)
            .expect("submission exists");
        submission.status = SubmissionStatus::Approved;
        submission.review = Some(ReviewDecision {
            approved: true,
            reason: "team work approved by founder review".into(),
            reviewed_at_unix: now_unix,
            reviewer: authority.founder_awe_id().clone(),
        });
        submission.paid_atoms = total;
        Ok(TeamPayoutReceipt {
            task_id,
            submission_id,
            recipients,
            paid_atoms: total,
            reward_id: root,
        })
    }

    /// The founder rejects submitted work; a reason is mandatory and no
    /// reward is minted.
    pub fn reject_submission(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        submission_id: u64,
        reason: &str,
        now_unix: u64,
    ) -> Result<(), WorkError> {
        let reason = reason.trim();
        if reason.is_empty() || reason.len() > MAX_REVIEW_REASON_CHARS {
            return Err(WorkError::InvalidReason);
        }
        authority.authorize(
            actor_public_key,
            FounderAction::ReviewSubmission,
            now_unix,
            "reject submission",
        )?;
        let submission = self
            .submissions
            .get_mut(&submission_id)
            .ok_or(WorkError::UnknownSubmission(submission_id))?;
        if !matches!(
            submission.status,
            SubmissionStatus::Submitted | SubmissionStatus::Revising
        ) {
            return Err(WorkError::NotAwaitingReview(submission_id));
        }
        submission.status = SubmissionStatus::Rejected;
        submission.review = Some(ReviewDecision {
            approved: false,
            reason: reason.to_string(),
            reviewed_at_unix: now_unix,
            reviewer: authority.founder_awe_id().clone(),
        });
        Ok(())
    }

    /// The founder requests changes; the contributor can resubmit.
    pub fn request_revision(
        &mut self,
        authority: &mut FounderAuthority,
        actor_public_key: &[u8; 32],
        submission_id: u64,
        reason: &str,
        now_unix: u64,
    ) -> Result<(), WorkError> {
        let reason = reason.trim();
        if reason.is_empty() || reason.len() > MAX_REVIEW_REASON_CHARS {
            return Err(WorkError::InvalidReason);
        }
        authority.authorize(
            actor_public_key,
            FounderAction::ReviewSubmission,
            now_unix,
            "request submission revision",
        )?;
        let submission = self
            .submissions
            .get_mut(&submission_id)
            .ok_or(WorkError::UnknownSubmission(submission_id))?;
        if !matches!(
            submission.status,
            SubmissionStatus::Submitted | SubmissionStatus::Revising
        ) {
            return Err(WorkError::NotAwaitingReview(submission_id));
        }
        submission.status = SubmissionStatus::Revising;
        submission.review = Some(ReviewDecision {
            approved: false,
            reason: reason.to_string(),
            reviewed_at_unix: now_unix,
            reviewer: authority.founder_awe_id().clone(),
        });
        Ok(())
    }

    /// The contributor replaces the evidence of a revising submission.
    pub fn resubmit(
        &mut self,
        contributor: &AweId,
        submission_id: u64,
        evidence: Vec<String>,
        now_unix: u64,
    ) -> Result<(), WorkError> {
        validate_evidence(&evidence)?;
        let submission = self
            .submissions
            .get_mut(&submission_id)
            .ok_or(WorkError::UnknownSubmission(submission_id))?;
        if submission.status != SubmissionStatus::Revising {
            return Err(WorkError::NotAwaitingReview(submission_id));
        }
        if submission.contributor != *contributor {
            return Err(WorkError::NotContributor(submission_id));
        }
        let task = self
            .tasks
            .get(&submission.task_id)
            .ok_or(WorkError::UnknownTask(submission.task_id))?;
        if task.status != WorkTaskStatus::Published {
            return Err(WorkError::TaskNotPublished(submission.task_id));
        }
        if let Some(deadline) = task.spec.deadline_unix {
            if now_unix > deadline {
                return Err(WorkError::DeadlinePassed(submission.task_id));
            }
        }
        submission.evidence = evidence;
        submission.status = SubmissionStatus::Submitted;
        submission.submitted_at_unix = now_unix;
        submission.review = None;
        Ok(())
    }

    /// The personal work page of a participant, built only from verified
    /// task and submission history.
    pub fn contributor_profile(&self, contributor: &AweId) -> WorkProfile {
        let key = contributor.to_hex();
        let mut profile = WorkProfile::default();
        for (task_id, assigned) in &self.assignments {
            if assigned.contains(&key) {
                profile.accepted_tasks.push(*task_id);
            }
        }
        for submission in self.submissions.values() {
            if submission.contributor == *contributor {
                profile.submissions.push(submission.id);
                match submission.status {
                    SubmissionStatus::Approved => {
                        profile.approved.push(submission.id);
                        profile.earned_atoms =
                            profile.earned_atoms.saturating_add(submission.paid_atoms);
                    }
                    SubmissionStatus::Rejected => profile.rejected.push(submission.id),
                    SubmissionStatus::Revising => profile.revising.push(submission.id),
                    SubmissionStatus::Submitted => {}
                }
            }
        }
        profile
    }

    pub fn contributor_level(&self, contributor: &AweId) -> ContributorLevel {
        ContributorLevel::from_approved(self.contributor_profile(contributor).approved.len())
    }
}

impl Default for WorkMarket {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;
    use crate::onecoin::ATOMS_PER_COIN;

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    fn ledger_for(founder: &Identity, members: &[AweId]) -> OnecoinLedger {
        let mut ledger = OnecoinLedger::default();
        let mut all = vec![founder.public.awe_id.clone()];
        all.extend(members.iter().cloned());
        ledger
            .initialize_genesis_with_verifiers(&all, &[founder.public.public_key])
            .unwrap();
        ledger
    }

    fn spec(title: &str, reward: u128) -> WorkTaskSpec {
        WorkTaskSpec {
            title: title.to_string(),
            description: format!("Implement {title} for AWENET"),
            category: WorkCategory::Engineering,
            complexity: Complexity::Intermediate,
            deadline_unix: None,
            required_skills: BTreeSet::new(),
            expected_outcome: "A reviewed, working result".to_string(),
            acceptance_criteria: vec!["All acceptance checks pass".to_string()],
            reward_atoms: reward,
            phases: Vec::new(),
            team_allowed: false,
            submission_format: "Repository patch with tests".to_string(),
        }
    }

    #[test]
    fn only_the_founder_publishes_official_tasks() {
        let founder = identity("founder");
        let member = identity("member");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        assert!(matches!(
            market
                .publish_task(
                    &mut authority,
                    &member.public.public_key,
                    spec("Task", 5),
                    1
                )
                .unwrap_err(),
            WorkError::Founder(FounderError::NotFounder)
        ));
        assert_eq!(market.task_count(), 0);
        // members may suggest, but suggestions are not official tasks
        let suggestion_id = market
            .suggest_task(&member.public.awe_id, spec("Suggested", 5), 2)
            .unwrap();
        assert_eq!(market.find_tasks(&TaskFilter::default()).len(), 0);
        let task_id = market
            .approve_suggestion(&mut authority, &founder.public.public_key, suggestion_id, 3)
            .unwrap();
        assert_eq!(
            market.task(task_id).unwrap().status,
            WorkTaskStatus::Published
        );
        assert!(matches!(
            market
                .approve_suggestion(&mut authority, &founder.public.public_key, suggestion_id, 4)
                .unwrap_err(),
            WorkError::SuggestionAlreadyApproved(_)
        ));
        assert!(market.suggestion(suggestion_id).unwrap().approved);
    }

    #[test]
    fn drafts_are_invisible_until_published() {
        let founder = identity("founder");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let task_id = market
            .create_draft_task(
                &mut authority,
                &founder.public.public_key,
                spec("Draft", 5),
                1,
            )
            .unwrap();
        assert_eq!(market.task(task_id).unwrap().status, WorkTaskStatus::Draft);
        let published_only = TaskFilter {
            status: Some(WorkTaskStatus::Published),
            ..TaskFilter::default()
        };
        assert_eq!(market.find_tasks(&published_only).len(), 0);
        market
            .publish_draft(&mut authority, &founder.public.public_key, task_id, 2)
            .unwrap();
        assert_eq!(market.find_tasks(&published_only).len(), 1);
        assert!(market.task(task_id).unwrap().published_at_unix.is_some());
    }

    #[test]
    fn reward_is_paid_once_and_only_after_founder_approval() {
        let founder = identity("founder");
        let worker = identity("worker");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let reward = ATOMS_PER_COIN;
        let task_id = market
            .publish_task(
                &mut authority,
                &founder.public.public_key,
                spec("Build module", reward),
                1,
            )
            .unwrap();
        let mut ledger = ledger_for(&founder, std::slice::from_ref(&worker.public.awe_id));
        market
            .accept_task(&worker.public.awe_id, task_id, 2)
            .unwrap();
        let submission_id = market
            .submit_work(
                &worker.public.awe_id,
                task_id,
                vec!["patch.rs".into()],
                None,
                3,
            )
            .unwrap();
        let receipt = market
            .approve_and_pay(&mut authority, &founder, &mut ledger, submission_id, 4)
            .unwrap();
        assert_eq!(receipt.paid_atoms, reward);
        assert_eq!(
            ledger.balance_atoms(&worker.public.awe_id),
            10 * ATOMS_PER_COIN + reward
        );
        // the same submission cannot be paid twice
        assert!(market
            .approve_and_pay(&mut authority, &founder, &mut ledger, submission_id, 5)
            .is_err());
        assert_eq!(
            ledger.balance_atoms(&worker.public.awe_id),
            10 * ATOMS_PER_COIN + reward
        );
    }

    #[test]
    fn rejection_needs_a_reason_and_pays_nothing() {
        let founder = identity("founder");
        let worker = identity("worker");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let task_id = market
            .publish_task(
                &mut authority,
                &founder.public.public_key,
                spec("Fix bug", 5),
                1,
            )
            .unwrap();
        let ledger = ledger_for(&founder, std::slice::from_ref(&worker.public.awe_id));
        market
            .accept_task(&worker.public.awe_id, task_id, 2)
            .unwrap();
        let submission_id = market
            .submit_work(
                &worker.public.awe_id,
                task_id,
                vec!["fix.rs".into()],
                None,
                3,
            )
            .unwrap();
        assert!(matches!(
            market
                .reject_submission(
                    &mut authority,
                    &founder.public.public_key,
                    submission_id,
                    "  ",
                    4
                )
                .unwrap_err(),
            WorkError::InvalidReason
        ));
        market
            .reject_submission(
                &mut authority,
                &founder.public.public_key,
                submission_id,
                "tests fail",
                4,
            )
            .unwrap();
        assert_eq!(
            market.submission(submission_id).unwrap().status,
            SubmissionStatus::Rejected
        );
        assert_eq!(
            market
                .submission(submission_id)
                .unwrap()
                .review
                .as_ref()
                .unwrap()
                .reason,
            "tests fail"
        );
        assert_eq!(
            ledger.balance_atoms(&worker.public.awe_id),
            10 * ATOMS_PER_COIN
        );
    }

    #[test]
    fn revising_submissions_can_be_resubmitted_by_their_contributor() {
        let founder = identity("founder");
        let worker = identity("worker");
        let other = identity("other-worker");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let task_id = market
            .publish_task(
                &mut authority,
                &founder.public.public_key,
                spec("Polish UI", 5),
                1,
            )
            .unwrap();
        let mut ledger = ledger_for(&founder, std::slice::from_ref(&worker.public.awe_id));
        market
            .accept_task(&worker.public.awe_id, task_id, 2)
            .unwrap();
        let submission_id = market
            .submit_work(
                &worker.public.awe_id,
                task_id,
                vec!["v1.png".into()],
                None,
                3,
            )
            .unwrap();
        market
            .request_revision(
                &mut authority,
                &founder.public.public_key,
                submission_id,
                "spacing is wrong",
                4,
            )
            .unwrap();
        assert_eq!(
            market.submission(submission_id).unwrap().status,
            SubmissionStatus::Revising
        );
        assert!(market
            .resubmit(
                &other.public.awe_id,
                submission_id,
                vec!["v2.png".into()],
                5
            )
            .is_err());
        market
            .resubmit(
                &worker.public.awe_id,
                submission_id,
                vec!["v2.png".into()],
                5,
            )
            .unwrap();
        assert_eq!(
            market.submission(submission_id).unwrap().status,
            SubmissionStatus::Submitted
        );
        market
            .approve_and_pay(&mut authority, &founder, &mut ledger, submission_id, 6)
            .unwrap();
        assert_eq!(
            ledger.balance_atoms(&worker.public.awe_id),
            10 * ATOMS_PER_COIN + 5
        );
    }

    #[test]
    fn deadlines_block_accepting_and_submitting() {
        let founder = identity("founder");
        let worker = identity("worker");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let mut s = spec("Timed task", 5);
        s.deadline_unix = Some(10);
        let task_id = market
            .publish_task(&mut authority, &founder.public.public_key, s, 1)
            .unwrap();
        market
            .accept_task(&worker.public.awe_id, task_id, 9)
            .unwrap();
        assert!(matches!(
            market
                .submit_work(
                    &worker.public.awe_id,
                    task_id,
                    vec!["late.rs".into()],
                    None,
                    11
                )
                .unwrap_err(),
            WorkError::DeadlinePassed(_)
        ));
        let late = identity("late-worker");
        assert!(matches!(
            market
                .accept_task(&late.public.awe_id, task_id, 11)
                .unwrap_err(),
            WorkError::DeadlinePassed(_)
        ));
    }

    #[test]
    fn contributors_must_accept_before_submitting() {
        let founder = identity("founder");
        let worker = identity("worker");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let task_id = market
            .publish_task(
                &mut authority,
                &founder.public.public_key,
                spec("Open task", 5),
                1,
            )
            .unwrap();
        assert!(matches!(
            market
                .submit_work(
                    &worker.public.awe_id,
                    task_id,
                    vec!["patch".into()],
                    None,
                    2
                )
                .unwrap_err(),
            WorkError::NotAssigned(_)
        ));
        market
            .accept_task(&worker.public.awe_id, task_id, 2)
            .unwrap();
        assert_eq!(market.assignment_count(task_id), 1);
        assert!(market
            .submit_work(
                &worker.public.awe_id,
                task_id,
                vec!["patch".into()],
                None,
                3
            )
            .is_ok());
    }

    #[test]
    fn phase_rewards_must_sum_to_the_task_reward() {
        let founder = identity("founder");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let mut s = spec("Phased project", 10);
        s.phases = vec![
            TaskPhase {
                name: "core".into(),
                acceptance_criteria: "core works".into(),
                reward_atoms: 6,
            },
            TaskPhase {
                name: "docs".into(),
                acceptance_criteria: "docs exist".into(),
                reward_atoms: 3,
            },
        ];
        assert!(matches!(
            market
                .publish_task(&mut authority, &founder.public.public_key, s.clone(), 1)
                .unwrap_err(),
            WorkError::PhaseRewardsMismatch
        ));
        s.phases[1].reward_atoms = 4;
        let task_id = market
            .publish_task(&mut authority, &founder.public.public_key, s, 2)
            .unwrap();
        assert_eq!(market.task(task_id).unwrap().spec.total_reward_atoms(), 10);
    }

    #[test]
    fn team_split_pays_exactly_the_published_reward() {
        let founder = identity("founder");
        let lead = identity("team-lead");
        let mate = identity("team-mate");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let mut s = spec("Team port", 3 * ATOMS_PER_COIN);
        s.team_allowed = true;
        let task_id = market
            .publish_task(&mut authority, &founder.public.public_key, s, 1)
            .unwrap();
        let mut ledger = ledger_for(
            &founder,
            &[lead.public.awe_id.clone(), mate.public.awe_id.clone()],
        );
        market.accept_task(&lead.public.awe_id, task_id, 2).unwrap();
        let submission_id = market
            .submit_work(
                &lead.public.awe_id,
                task_id,
                vec!["team.patch".into()],
                Some("lead submits for the team".into()),
                3,
            )
            .unwrap();
        // a split that does not sum to the reward is rejected
        assert!(matches!(
            market
                .approve_and_pay_team(
                    &mut authority,
                    &founder,
                    &mut ledger,
                    submission_id,
                    &[(lead.public.awe_id.clone(), ATOMS_PER_COIN)],
                    4
                )
                .unwrap_err(),
            WorkError::InvalidSplit
        ));
        let split = vec![
            (lead.public.awe_id.clone(), 2 * ATOMS_PER_COIN),
            (mate.public.awe_id.clone(), ATOMS_PER_COIN),
        ];
        let receipt = market
            .approve_and_pay_team(
                &mut authority,
                &founder,
                &mut ledger,
                submission_id,
                &split,
                5,
            )
            .unwrap();
        assert_eq!(receipt.paid_atoms, 3 * ATOMS_PER_COIN);
        assert_eq!(receipt.recipients.len(), 2);
        assert_eq!(
            ledger.balance_atoms(&lead.public.awe_id),
            12 * ATOMS_PER_COIN
        );
        assert_eq!(
            ledger.balance_atoms(&mate.public.awe_id),
            11 * ATOMS_PER_COIN
        );
        // the paid submission cannot be paid again
        assert!(market
            .approve_and_pay(&mut authority, &founder, &mut ledger, submission_id, 6)
            .is_err());
    }

    #[test]
    fn solo_payout_on_a_non_team_task_is_rejected() {
        let founder = identity("founder");
        let lead = identity("solo-lead");
        let mate = identity("solo-mate");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let task_id = market
            .publish_task(
                &mut authority,
                &founder.public.public_key,
                spec("Solo task", 5),
                1,
            )
            .unwrap();
        let mut ledger = ledger_for(
            &founder,
            &[lead.public.awe_id.clone(), mate.public.awe_id.clone()],
        );
        market.accept_task(&lead.public.awe_id, task_id, 2).unwrap();
        let submission_id = market
            .submit_work(
                &lead.public.awe_id,
                task_id,
                vec!["solo.rs".into()],
                None,
                3,
            )
            .unwrap();
        assert!(matches!(
            market
                .approve_and_pay_team(
                    &mut authority,
                    &founder,
                    &mut ledger,
                    submission_id,
                    &[
                        (lead.public.awe_id.clone(), 3),
                        (mate.public.awe_id.clone(), 2)
                    ],
                    4
                )
                .unwrap_err(),
            WorkError::TeamWorkNotAllowed(_)
        ));
    }

    #[test]
    fn find_tasks_filters_by_category_status_and_reward() {
        let founder = identity("founder");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let mut small = spec("Small fix", 1);
        small.category = WorkCategory::Design;
        let mut big = spec("Big feature", 100);
        big.category = WorkCategory::Engineering;
        market
            .publish_task(&mut authority, &founder.public.public_key, small, 1)
            .unwrap();
        market
            .publish_task(&mut authority, &founder.public.public_key, big, 2)
            .unwrap();
        let engineering = TaskFilter {
            category: Some(WorkCategory::Engineering),
            ..TaskFilter::default()
        };
        assert_eq!(market.find_tasks(&engineering).len(), 1);
        assert_eq!(market.find_tasks(&engineering)[0].spec.title, "Big feature");
        let rich = TaskFilter {
            min_reward_atoms: 50,
            ..TaskFilter::default()
        };
        assert_eq!(market.find_tasks(&rich).len(), 1);
        let published = TaskFilter {
            status: Some(WorkTaskStatus::Published),
            ..TaskFilter::default()
        };
        assert_eq!(market.find_tasks(&published).len(), 2);
    }

    #[test]
    fn profiles_and_levels_reflect_real_approved_work() {
        assert_eq!(
            ContributorLevel::from_approved(0),
            ContributorLevel::Beginner
        );
        assert_eq!(
            ContributorLevel::from_approved(2),
            ContributorLevel::Contributor
        );
        assert_eq!(
            ContributorLevel::from_approved(4),
            ContributorLevel::Advanced
        );
        assert_eq!(ContributorLevel::from_approved(7), ContributorLevel::Expert);
        assert_eq!(
            ContributorLevel::from_approved(12),
            ContributorLevel::Master
        );

        let founder = identity("founder");
        let worker = identity("worker");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let mut ledger = ledger_for(&founder, std::slice::from_ref(&worker.public.awe_id));
        let task_id = market
            .publish_task(
                &mut authority,
                &founder.public.public_key,
                spec("First task", 5),
                1,
            )
            .unwrap();
        market
            .accept_task(&worker.public.awe_id, task_id, 2)
            .unwrap();
        let submission_id = market
            .submit_work(
                &worker.public.awe_id,
                task_id,
                vec!["one.rs".into()],
                None,
                3,
            )
            .unwrap();
        let before = market.contributor_profile(&worker.public.awe_id);
        assert_eq!(before.accepted_tasks, vec![task_id]);
        assert_eq!(before.submissions, vec![submission_id]);
        assert_eq!(
            market.contributor_level(&worker.public.awe_id),
            ContributorLevel::Beginner
        );
        market
            .approve_and_pay(&mut authority, &founder, &mut ledger, submission_id, 4)
            .unwrap();
        let after = market.contributor_profile(&worker.public.awe_id);
        assert_eq!(after.approved, vec![submission_id]);
        assert_eq!(after.earned_atoms, 5);
        assert_eq!(
            market.contributor_level(&worker.public.awe_id),
            ContributorLevel::Contributor
        );
    }

    #[test]
    fn closed_tasks_stop_accepting_work() {
        let founder = identity("founder");
        let worker = identity("worker");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let task_id = market
            .publish_task(
                &mut authority,
                &founder.public.public_key,
                spec("Closing", 5),
                1,
            )
            .unwrap();
        market
            .close_task(&mut authority, &founder.public.public_key, task_id, 2)
            .unwrap();
        assert!(matches!(
            market
                .accept_task(&worker.public.awe_id, task_id, 3)
                .unwrap_err(),
            WorkError::TaskNotPublished(_)
        ));
        assert!(matches!(
            market
                .submit_work(&worker.public.awe_id, task_id, vec!["late".into()], None, 3)
                .unwrap_err(),
            WorkError::TaskNotPublished(_)
        ));
        assert_eq!(market.task(task_id).unwrap().status, WorkTaskStatus::Closed);
        // a closed task cannot be modified or closed again
        assert!(market
            .modify_task(
                &mut authority,
                &founder.public.public_key,
                task_id,
                spec("New", 6),
                4
            )
            .is_err());
        assert!(market
            .close_task(&mut authority, &founder.public.public_key, task_id, 4)
            .is_err());
        assert_eq!(
            market.task(task_id).unwrap().publisher(),
            &founder.public.awe_id
        );
    }

    #[test]
    fn submission_and_suggestion_queues_are_iterable() {
        let founder = identity("iter-founder");
        let contributor = identity("iter-contributor");
        let mut authority = FounderAuthority::install(&founder, 0);
        let mut market = WorkMarket::new();
        let task = market
            .publish_task(
                &mut authority,
                &founder.public.public_key,
                spec("Queue task", 10 * ATOMS_PER_COIN),
                1,
            )
            .unwrap();
        let suggestion = market
            .suggest_task(
                &contributor.public.awe_id,
                spec("Suggested task", 5 * ATOMS_PER_COIN),
                2,
            )
            .unwrap();
        market
            .accept_task(&contributor.public.awe_id, task, 3)
            .unwrap();
        let submission = market
            .submit_work(
                &contributor.public.awe_id,
                task,
                vec!["https://example.com/proof".to_string()],
                None,
                4,
            )
            .unwrap();
        let submissions: Vec<_> = market.submissions().collect();
        assert_eq!(submissions.len(), 1);
        assert_eq!(submissions[0].id, submission);
        assert_eq!(submissions[0].task_id, task);
        let suggestions: Vec<_> = market.suggestions().collect();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].id, suggestion);
        assert_eq!(suggestions[0].contributor, contributor.public.awe_id);
    }
}

