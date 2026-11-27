//! Approval cards and tool execution for the session.

use std::path::{Path, PathBuf};

use serde_json::json;
use twitty_core::redact::redact;

use super::*;
use crate::engine::patch::PatchError;
use crate::engine::policy::{self, Approval};
use crate::engine::tools::{Tool, MAX_READ_BYTES};

fn read_failed() -> &'static str {
    twitty_core::language::tr(
        "Read failed or exceeded its limit",
        "تعذّرت القراءة أو تجاوزت الحد",
    )
}
fn denied() -> &'static str {
    twitty_core::language::tr("Denied by policy", "رُفض بموجب السياسة")
}
fn user_denied() -> &'static str {
    twitty_core::language::tr("Denied by the user", "رفض المستخدم الطلب")
}
fn edited_note() -> &'static str {
    twitty_core::language::tr("User-edited", "عدّله المستخدم")
}
fn changed() -> &'static str {
    twitty_core::language::tr(
        "The target changed after approval",
        "تغيّر الهدف بعد الموافقة",
    )
}

pub(super) enum Action {
    Read { tool: Tool, resolved: PathBuf },
    Command,
    Patch(Patch),
    Fact,
}

impl Session {
    pub(super) fn propose(&mut self, call: ToolCall, config: &EngineConfig, events: &Events) {
        self.record(
            json!({"event":"tool_call","id":call.id,"tool":call.name,"arguments":call.arguments}),
        );
        if !self.journal_ok {
            self.calls.push_front(call);
            return;
        }
        let Some(tool) = call.decode() else {
            self.add(
                StepKind::Message {
                    text: invalid_tool().into(),
                },
                StepStatus::Failed(invalid_tool().into()),
                events,
            );
            self.result(call, invalid_tool());
            return;
        };
        if let Some((kind, target, path)) = tool.read_kind() {
            return self.propose_read(call, tool.clone(), kind, target, path, events);
        }
        match tool {
            Tool::RunCommand { command, reason } => {
                let kind = StepKind::Command {
                    command: command.clone(),
                    cwd: self.cwd.clone(),
                    reason: bounded(&clean(&reason), 1024).into(),
                    after_untrusted_data: self.tainted,
                };
                match self.command_policy(config, &command) {
                    Approval::Deny(reason) => self.deny(call, &command, reason, events),
                    Approval::Prompt => self.wait(call, kind, Action::Command, events),
                    Approval::Allow => {
                        self.record(json!({"event":"policy","tool":call.name,"decision":"allow"}));
                        let card = self.add(kind, StepStatus::Running, events);
                        self.dispatch(card, call, false, events);
                    }
                }
            }
            Tool::ApplyPatch { diff, reason } => match Patch::validate(&self.root, &diff) {
                Ok(patch) => {
                    let kind = StepKind::Patch {
                        files: patch.files(),
                        diff,
                        reason: bounded(&clean(&reason), 1024).into(),
                    };
                    self.wait(call, kind, Action::Patch(patch), events);
                }
                Err(PatchError::Policy(reason)) => self.deny(call, "apply_patch", reason, events),
                Err(error) => {
                    let message = patch_message(&error);
                    self.add(
                        StepKind::Message {
                            text: message.into(),
                        },
                        StepStatus::Failed(message.into()),
                        events,
                    );
                    self.result(call, message);
                }
            },
            Tool::RememberFact { text } => {
                let text = bounded(&clean(&text), 4096).trim().to_owned();
                if text.is_empty() {
                    self.result(call, invalid_tool());
                } else {
                    self.wait(
                        call,
                        StepKind::FactSuggestion { text },
                        Action::Fact,
                        events,
                    );
                }
            }
            _ => unreachable!("reads handled above"),
        }
    }

    fn propose_read(
        &mut self,
        call: ToolCall,
        tool: Tool,
        kind: ReadTool,
        target: String,
        path: Option<&Path>,
        events: &Events,
    ) {
        match path.map(|path| policy::path_policy(&self.root, path, false)) {
            Some(Err(reason)) => self.deny(call, &target, reason, events),
            Some(Ok((resolved, Approval::Prompt))) => {
                let target = if path.is_some_and(|path| self.root.join(path) != resolved) {
                    format!("{target} → {}", resolved.display())
                } else {
                    target
                };
                self.wait(
                    call,
                    StepKind::ReadApproval {
                        tool: kind,
                        target: clean(&target),
                    },
                    Action::Read { tool, resolved },
                    events,
                );
            }
            resolved => {
                self.record(json!({"event":"policy","tool":call.name,"decision":"allow"}));
                let resolved = resolved.and_then(Result::ok).map(|(path, _)| path);
                self.execute_read(call, &tool, resolved.as_deref(), events);
            }
        }
    }

    fn execute_read(
        &mut self,
        call: ToolCall,
        tool: &Tool,
        resolved: Option<&Path>,
        events: &Events,
    ) {
        let (kind, target, _) = tool.read_kind().expect("read tool");
        let result = tools::read(tool, &self.root, resolved, &self.context);
        let (content, status) = match result {
            Some(text) => (
                bounded(&clean(&text), MAX_READ_BYTES).into(),
                StepStatus::Done,
            ),
            None => (
                read_failed().to_owned(),
                StepStatus::Failed(read_failed().into()),
            ),
        };
        self.add(
            StepKind::Read {
                tool: kind,
                target: clean(&target),
                preview: bounded(&content, 4096).into(),
            },
            status,
            events,
        );
        self.result(call, &content);
    }

    fn command_policy(&self, config: &EngineConfig, command: &str) -> Approval {
        if command.len() > 16 * 1024 || redact(command) != command {
            return Approval::Deny(DenyReason::HardDeny {
                rule: "invalid or sensitive command".into(),
            });
        }
        let prefixes = config
            .allow_lists
            .get(&self.root)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        policy::command(&self.root, &self.cwd, command, prefixes, self.tainted)
    }

    fn deny(&mut self, call: ToolCall, what: &str, reason: DenyReason, events: &Events) {
        self.record(json!({"event":"policy","tool":call.name,"decision":"deny","reason":format!("{reason:?}")}));
        self.add(
            StepKind::Denied {
                what: clean(what),
                reason,
            },
            StepStatus::Denied,
            events,
        );
        self.result(call, denied());
    }

    fn wait(&mut self, call: ToolCall, kind: StepKind, action: Action, events: &Events) {
        self.record(json!({"event":"policy","tool":call.name,"decision":"prompt"}));
        let card = self.add(kind, StepStatus::WaitingApproval, events);
        self.pending = Some(Pending::Approval { card, call, action });
    }

    pub(in crate::engine) fn decide(
        &mut self,
        step: StepId,
        decision: Decision,
        config: &EngineConfig,
        events: &Events,
    ) {
        let matches_waiting = matches!(&self.pending,
            Some(Pending::Approval { card, .. } | Pending::Limit { card, .. }) if card.id == step);
        if !matches_waiting {
            return;
        }
        let decision_log = match &decision {
            Decision::Approve => json!({"action":"approve"}),
            Decision::ApproveEdited(text) => json!({"action":"edit","text":text}),
            Decision::Deny => json!({"action":"deny"}),
        };
        self.record(json!({"event":"decision","step":step.0,"decision":decision_log}));
        if !self.journal_ok {
            return;
        }

        match self.pending.take() {
            Some(Pending::Limit { mut card, kind }) => {
                if matches!(decision, Decision::Approve) {
                    card.status = StepStatus::Done;
                    self.allow_limit(kind);
                    self.update(&card, events);
                } else {
                    card.status = StepStatus::Denied;
                    self.update(&card, events);
                    self.stop(events);
                }
            }
            Some(Pending::Approval {
                mut card,
                call,
                action,
            }) => match decision {
                Decision::Deny => {
                    card.status = StepStatus::Denied;
                    self.update(&card, events);
                    self.add(
                        StepKind::Denied {
                            what: call.name.clone(),
                            reason: DenyReason::User,
                        },
                        StepStatus::Denied,
                        events,
                    );
                    self.result(call, user_denied());
                }
                decision => self.approve(card, call, action, decision, config, events),
            },
            _ => unreachable!("waiting step checked above"),
        }
    }

    fn approve(
        &mut self,
        mut card: Step,
        call: ToolCall,
        action: Action,
        decision: Decision,
        config: &EngineConfig,
        events: &Events,
    ) {
        match (action, decision) {
            (Action::Read { tool, resolved }, Decision::Approve) => {
                let current = tool
                    .read_kind()
                    .and_then(|(_, _, path)| path)
                    .and_then(|path| policy::path_policy(&self.root, path, false).ok());
                if !current.is_some_and(|(path, _)| path == resolved) {
                    self.fail(card, call, changed(), events);
                    return;
                }
                card.status = StepStatus::Done;
                self.update(&card, events);
                self.execute_read(call, &tool, Some(&resolved), events);
            }
            (Action::Command, decision) => {
                let StepKind::Command { command, .. } = &mut card.kind else {
                    unreachable!("command card")
                };
                let edited = if let Decision::ApproveEdited(text) = decision {
                    *command = text;
                    true
                } else {
                    false
                };
                if let Approval::Deny(reason) = self.command_policy(config, command) {
                    card.status = StepStatus::Denied;
                    self.add(
                        StepKind::Denied {
                            what: clean(command),
                            reason: reason.clone(),
                        },
                        StepStatus::Denied,
                        events,
                    );
                    self.record(json!({"event":"policy","tool":call.name,"decision":"deny","reason":format!("{reason:?}")}));
                    *command = clean(command);
                    self.update(&card, events);
                    self.result(call, denied());
                    return;
                }
                card.status = StepStatus::Running;
                self.update(&card, events);
                self.dispatch(card, call, edited, events);
            }
            (Action::Patch(mut patch), decision) => {
                let edited = if let Decision::ApproveEdited(diff) = decision {
                    match Patch::validate(&self.root, &diff) {
                        Ok(new_patch) => patch = new_patch,
                        Err(error) => {
                            if let PatchError::Policy(reason) = &error {
                                self.add(
                                    StepKind::Denied {
                                        what: "apply_patch".into(),
                                        reason: reason.clone(),
                                    },
                                    StepStatus::Denied,
                                    events,
                                );
                            }
                            return self.fail(card, call, patch_message(&error), events);
                        }
                    }
                    if let StepKind::Patch {
                        files,
                        diff: displayed,
                        ..
                    } = &mut card.kind
                    {
                        *files = patch.files();
                        *displayed = redact(&diff).into_owned();
                    }
                    true
                } else {
                    false
                };
                self.apply(card, call, patch, edited, events);
            }
            (Action::Fact, Decision::Approve) => {
                let StepKind::FactSuggestion { text } = &card.kind else {
                    unreachable!("fact card")
                };
                let _ = events.send(AgentEvent::RememberFact {
                    session: self.id,
                    project_root: self.project_root.clone(),
                    text: text.clone(),
                });
                card.status = StepStatus::Done;
                self.update(&card, events);
                self.result(
                    call,
                    twitty_core::language::tr("Fact approved", "وافق المستخدم على المعلومة"),
                );
            }
            _ => self.fail(
                card,
                call,
                twitty_core::language::tr("This card cannot be edited", "لا يمكن تعديل هذه البطاقة"),
                events,
            ),
        }
    }

    fn fail(&mut self, mut card: Step, call: ToolCall, message: &str, events: &Events) {
        card.status = StepStatus::Failed(message.into());
        self.update(&card, events);
        self.result(call, message);
    }

    fn dispatch(&mut self, card: Step, call: ToolCall, edited: bool, events: &Events) {
        if !self.journal_ok {
            self.fail(card, call, journal_error(), events);
            return;
        }
        if self.cwd.canonicalize().ok().as_ref() != Some(&self.cwd) || !self.cwd.is_dir() {
            self.fail(card, call, folder_error(), events);
            return;
        }
        let StepKind::Command { command, cwd, .. } = &card.kind else {
            unreachable!("command card")
        };
        self.record(json!({"event":"run_in_pane","step":card.id.0,"command":command,"cwd":cwd.to_string_lossy(),"edited":edited}));
        if !self.journal_ok {
            self.fail(card, call, journal_error(), events);
            return;
        }
        let _ = events.send(AgentEvent::RunInPane {
            session: self.id,
            step: card.id,
            command: command.clone(),
            cwd: cwd.clone(),
        });
        self.pending = Some(Pending::Command { card, call, edited });
    }

    pub(in crate::engine) fn command_finished(
        &mut self,
        step: StepId,
        exit: Option<i32>,
        duration_ms: u64,
        output: &str,
        events: &Events,
    ) {
        if !matches!(&self.pending, Some(Pending::Command { card, .. }) if card.id == step) {
            return;
        }
        let Some(Pending::Command {
            mut card,
            call,
            edited,
        }) = self.pending.take()
        else {
            return;
        };
        let output = clean(output);
        let tail = crate::ai::tail_lines(&output, crate::ai::MAX_BLOCK_OUTPUT_LINES);
        let tail = bounded(&tail, MAX_READ_BYTES).to_owned();
        let (status, summary) = match exit {
            Some(0) => (
                StepStatus::Done,
                twitty_core::language::tr("Command succeeded", "نجح الأمر"),
            ),
            Some(_) => (
                StepStatus::Failed(twitty_core::language::tr("Command failed", "فشل الأمر").into()),
                twitty_core::language::tr("Command failed", "فشل الأمر"),
            ),
            None => (
                StepStatus::Cancelled,
                twitty_core::language::tr("Command interrupted", "توقّف الأمر"),
            ),
        };
        let edited_text = if edited {
            if let StepKind::Command { command, .. } = &card.kind {
                format!("\n{}\n{command}", edited_note())
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        card.status = status;
        self.update(&card, events);
        self.add(
            StepKind::CommandResult {
                exit,
                duration_ms,
                tail: tail.clone(),
            },
            StepStatus::Done,
            events,
        );
        self.result(call, &format!("{summary}\nExit status: {exit:?}\nDuration (ms): {duration_ms}{edited_text}\n{tail}"));
    }

    fn apply(
        &mut self,
        mut card: Step,
        call: ToolCall,
        patch: Patch,
        edited: bool,
        events: &Events,
    ) {
        self.record(
            json!({"event":"patch_undo_data","step":card.id.0,"files":patch.undo_record()}),
        );
        if !self.journal_ok {
            self.fail(card, call, journal_error(), events);
            return;
        }
        if let Err(error) = patch.apply() {
            self.fail(card, call, patch_message(&error), events);
            return;
        }
        card.status = StepStatus::Done;
        self.update(&card, events);
        let files = patch.files().into_iter().map(|file| file.path).collect();
        let applied = self.add(
            StepKind::PatchApplied {
                files,
                undo_available: true,
            },
            StepStatus::Done,
            events,
        );
        self.record(json!({"event":"patch_applied","proposal":card.id.0,"step":applied.id.0}));
        self.patches.insert(applied.id, (applied, patch));
        let notes = if edited {
            if let StepKind::Patch { diff, .. } = &card.kind {
                format!("\n{}\n{diff}", edited_note())
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        self.result(
            call,
            &format!(
                "{}{notes}",
                twitty_core::language::tr("Patch applied", "طُبّق التعديل")
            ),
        );
    }

    pub(in crate::engine) fn undo(&mut self, id: StepId, events: &Events) {
        if !self.patches.contains_key(&id) {
            return;
        }
        self.record(json!({"event":"undo_requested","step":id.0}));
        if !self.journal_ok {
            self.add(
                StepKind::Message {
                    text: journal_error().into(),
                },
                StepStatus::Failed(journal_error().into()),
                events,
            );
            return;
        }
        let Some((mut card, patch)) = self.patches.remove(&id) else {
            return;
        };
        let result = patch.undo();
        let undo_available = matches!(&result, Err(PatchError::Io));
        if let StepKind::PatchApplied {
            undo_available: available,
            ..
        } = &mut card.kind
        {
            *available = undo_available;
        }
        let message = match &result {
            Ok(()) => twitty_core::language::tr("Patch undone", "أُلغي التعديل"),
            Err(error) => patch_message(error),
        };
        self.record(json!({"event":"undo_result","step":id.0,"message":message}));
        self.update(&card, events);
        let status = if result.is_ok() {
            StepStatus::Done
        } else {
            StepStatus::Failed(message.into())
        };
        self.add(
            StepKind::Message {
                text: message.into(),
            },
            status,
            events,
        );
        if undo_available {
            self.patches.insert(id, (card, patch));
        }
    }
}

fn patch_message(error: &PatchError) -> &'static str {
    match error {
        PatchError::Invalid => twitty_core::language::tr(
            "Invalid or oversized unified diff",
            "فرق موحّد غير صالح أو أكبر من الحد",
        ),
        PatchError::Policy(_) => denied(),
        PatchError::Conflict => changed(),
        PatchError::Io => twitty_core::language::tr("Patch write failed", "تعذّرت كتابة التعديل"),
        PatchError::Rollback => {
            twitty_core::language::tr("Patch rollback incomplete", "لم يكتمل التراجع عن التعديل")
        }
    }
}
