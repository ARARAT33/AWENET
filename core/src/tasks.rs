//! Bounded task queue with priorities, deadlines and a retry budget.
//!
//! Deterministic and replay-friendly: pure data, no wall clock and no
//! threads. Time is an explicit tick supplied by the caller. Tasks carry a
//! priority and an optional deadline; `next_ready` picks the highest
//! priority unexpired pending task, and `fail` either schedules a retry or
//! dead-ends the task once its attempt budget is spent.

use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_TASKS: usize = 10_000;
pub const MAX_ATTEMPTS: u32 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum TaskError {
    #[error("task queue capacity must be between 1 and {MAX_TASKS}")]
    InvalidCapacity,
    #[error("task queue is full (max {MAX_TASKS} tasks)")]
    QueueFull,
    #[error("max attempts must be between 1 and {MAX_ATTEMPTS}")]
    InvalidMaxAttempts,
    #[error("unknown task {0}")]
    UnknownTask(u64),
    #[error("task {0} is not pending")]
    NotPending(u64),
    #[error("task {0} is not running")]
    NotRunning(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct Task {
    pub id: u64,
    pub label: String,
    pub priority: Priority,
    pub state: TaskState,
    pub created_tick: u64,
    pub deadline_tick: Option<u64>,
    pub attempts: u32,
    pub max_attempts: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailOutcome {
    Retried { attempts_left: u32 },
    Exhausted { attempts: u32 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskStats {
    pub pending: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}

pub struct TaskQueue {
    max_tasks: usize,
    next_id: u64,
    tasks: BTreeMap<u64, Task>,
}

impl TaskQueue {
    pub fn new(max_tasks: usize) -> Result<Self, TaskError> {
        if max_tasks == 0 || max_tasks > MAX_TASKS {
            return Err(TaskError::InvalidCapacity);
        }
        Ok(Self {
            max_tasks,
            next_id: 1,
            tasks: BTreeMap::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Task> {
        self.tasks.get(&id)
    }

    /// Submits a new pending task and returns its id.
    pub fn submit(
        &mut self,
        label: &str,
        priority: Priority,
        created_tick: u64,
        deadline_tick: Option<u64>,
        max_attempts: u32,
    ) -> Result<u64, TaskError> {
        if max_attempts == 0 || max_attempts > MAX_ATTEMPTS {
            return Err(TaskError::InvalidMaxAttempts);
        }
        if self.tasks.len() >= self.max_tasks {
            return Err(TaskError::QueueFull);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.insert(
            id,
            Task {
                id,
                label: label.to_string(),
                priority,
                state: TaskState::Pending,
                created_tick,
                deadline_tick,
                attempts: 0,
                max_attempts,
            },
        );
        Ok(id)
    }

    /// Picks the highest priority pending task that has not expired.
    pub fn next_ready(&self, now: u64) -> Option<&Task> {
        self.tasks
            .values()
            .filter(|task| task.state == TaskState::Pending && !Self::expired(task, now))
            .max_by(|a, b| a.priority.cmp(&b.priority).then(b.id.cmp(&a.id)))
    }

    /// Moves a pending, unexpired task to Running and consumes one attempt.
    pub fn start(&mut self, id: u64, now: u64) -> Result<(), TaskError> {
        let task = self.tasks.get_mut(&id).ok_or(TaskError::UnknownTask(id))?;
        if task.state != TaskState::Pending {
            return Err(TaskError::NotPending(id));
        }
        if Self::expired(task, now) {
            task.state = TaskState::Failed;
            return Err(TaskError::NotPending(id));
        }
        task.state = TaskState::Running;
        task.attempts += 1;
        Ok(())
    }

    pub fn complete(&mut self, id: u64) -> Result<(), TaskError> {
        let task = self.tasks.get_mut(&id).ok_or(TaskError::UnknownTask(id))?;
        if task.state != TaskState::Running {
            return Err(TaskError::NotRunning(id));
        }
        task.state = TaskState::Completed;
        Ok(())
    }

    /// Records a failure for a running task: requeues it when the attempt
    /// budget allows, otherwise dead-ends it as Failed.
    pub fn fail(&mut self, id: u64) -> Result<FailOutcome, TaskError> {
        let task = self.tasks.get_mut(&id).ok_or(TaskError::UnknownTask(id))?;
        if task.state != TaskState::Running {
            return Err(TaskError::NotRunning(id));
        }
        let attempts_left = task.max_attempts - task.attempts;
        if attempts_left > 0 {
            task.state = TaskState::Pending;
            Ok(FailOutcome::Retried { attempts_left })
        } else {
            task.state = TaskState::Failed;
            Ok(FailOutcome::Exhausted {
                attempts: task.attempts,
            })
        }
    }

    pub fn cancel(&mut self, id: u64) -> Result<(), TaskError> {
        let task = self.tasks.get_mut(&id).ok_or(TaskError::UnknownTask(id))?;
        if task.state != TaskState::Pending {
            return Err(TaskError::NotPending(id));
        }
        task.state = TaskState::Cancelled;
        Ok(())
    }

    /// Fails every pending task whose deadline has passed and returns ids.
    pub fn expire(&mut self, now: u64) -> Vec<u64> {
        let mut expired = Vec::new();
        for task in self.tasks.values_mut() {
            if task.state == TaskState::Pending && Self::expired(task, now) {
                task.state = TaskState::Failed;
                expired.push(task.id);
            }
        }
        expired
    }

    pub fn stats(&self) -> TaskStats {
        let mut stats = TaskStats::default();
        for task in self.tasks.values() {
            match task.state {
                TaskState::Pending => stats.pending += 1,
                TaskState::Running => stats.running += 1,
                TaskState::Completed => stats.completed += 1,
                TaskState::Failed => stats.failed += 1,
                TaskState::Cancelled => stats.cancelled += 1,
            }
        }
        stats
    }

    fn expired(task: &Task, now: u64) -> bool {
        matches!(task.deadline_tick, Some(deadline) if deadline <= now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_capacity() {
        assert_eq!(TaskQueue::new(0).unwrap_err(), TaskError::InvalidCapacity);
        assert_eq!(
            TaskQueue::new(MAX_TASKS + 1).unwrap_err(),
            TaskError::InvalidCapacity
        );
        assert!(TaskQueue::new(1).is_ok());
    }

    #[test]
    fn validates_max_attempts() {
        let mut queue = TaskQueue::new(4).unwrap();
        assert_eq!(
            queue.submit("t", Priority::Normal, 0, None, 0).unwrap_err(),
            TaskError::InvalidMaxAttempts
        );
        assert_eq!(
            queue
                .submit("t", Priority::Normal, 0, None, MAX_ATTEMPTS + 1)
                .unwrap_err(),
            TaskError::InvalidMaxAttempts
        );
    }

    #[test]
    fn queue_full_is_rejected() {
        let mut queue = TaskQueue::new(1).unwrap();
        queue.submit("a", Priority::Low, 0, None, 3).unwrap();
        assert_eq!(
            queue.submit("b", Priority::Low, 0, None, 3).unwrap_err(),
            TaskError::QueueFull
        );
    }

    #[test]
    fn next_ready_prefers_highest_priority() {
        let mut queue = TaskQueue::new(8).unwrap();
        queue.submit("low", Priority::Low, 0, None, 3).unwrap();
        queue
            .submit("critical", Priority::Critical, 0, None, 3)
            .unwrap();
        queue.submit("high", Priority::High, 0, None, 3).unwrap();
        let next = queue.next_ready(1).unwrap();
        assert_eq!(next.label, "critical");
        queue.start(next.id, 1).unwrap();
        let next = queue.next_ready(1).unwrap();
        assert_eq!(next.label, "high");
    }

    #[test]
    fn start_complete_lifecycle() {
        let mut queue = TaskQueue::new(4).unwrap();
        let id = queue.submit("job", Priority::Normal, 0, None, 3).unwrap();
        assert_eq!(queue.get(id).unwrap().state, TaskState::Pending);
        queue.start(id, 5).unwrap();
        assert_eq!(queue.get(id).unwrap().attempts, 1);
        assert_eq!(queue.get(id).unwrap().state, TaskState::Running);
        queue.complete(id).unwrap();
        assert_eq!(queue.get(id).unwrap().state, TaskState::Completed);
    }

    #[test]
    fn fail_requeues_within_budget() {
        let mut queue = TaskQueue::new(4).unwrap();
        let id = queue.submit("job", Priority::Normal, 0, None, 2).unwrap();
        queue.start(id, 0).unwrap();
        assert_eq!(
            queue.fail(id).unwrap(),
            FailOutcome::Retried { attempts_left: 1 }
        );
        assert_eq!(queue.get(id).unwrap().state, TaskState::Pending);
        queue.start(id, 1).unwrap();
        assert_eq!(
            queue.fail(id).unwrap(),
            FailOutcome::Exhausted { attempts: 2 }
        );
        assert_eq!(queue.get(id).unwrap().state, TaskState::Failed);
    }

    #[test]
    fn deadlines_expire_pending_tasks() {
        let mut queue = TaskQueue::new(4).unwrap();
        let id = queue
            .submit("late", Priority::Normal, 0, Some(10), 3)
            .unwrap();
        assert!(queue.next_ready(10).is_none());
        let id_alive = queue
            .submit("ok", Priority::Normal, 0, Some(100), 3)
            .unwrap();
        assert!(queue.next_ready(10).is_some());
        assert_eq!(queue.expire(10), vec![id]);
        assert_eq!(queue.get(id).unwrap().state, TaskState::Failed);
        assert_eq!(queue.get(id_alive).unwrap().state, TaskState::Pending);
    }

    #[test]
    fn cancel_and_stats() {
        let mut queue = TaskQueue::new(8).unwrap();
        let a = queue.submit("a", Priority::Low, 0, None, 3).unwrap();
        let b = queue.submit("b", Priority::Low, 0, None, 3).unwrap();
        queue.cancel(a).unwrap();
        queue.start(b, 0).unwrap();
        queue.complete(b).unwrap();
        let stats = queue.stats();
        assert_eq!(stats.cancelled, 1);
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.pending, 0);
        assert_eq!(queue.len(), 2);
    }
}
