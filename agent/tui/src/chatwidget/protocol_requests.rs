//! App-server request and notification dispatch for `ChatWidget`.
//!
//! This module translates protocol requests into the focused chat-widget flows
//! that render approvals, permissions, tool input, and guardian reviews.

use super::*;

impl ChatWidget {
    pub(crate) fn handle_server_request(
        &mut self,
        request: ServerRequest,
        replay_kind: Option<ReplayKind>,
    ) {
        let id = request.id().to_string();
        match request {
            ServerRequest::CommandExecutionRequestApproval { params, .. } => {
                let fallback_cwd = self.config.cwd.clone();
                self.on_exec_approval_request(
                    id,
                    exec_approval_request_from_params(params, &fallback_cwd),
                );
            }
            ServerRequest::FileChangeRequestApproval { params, .. } => {
                self.on_apply_patch_approval_request(
                    id,
                    patch_approval_request_from_params(params),
                );
            }
            ServerRequest::McpServerElicitationRequest { request_id, params } => {
                self.on_elicitation_request(request_id, params);
            }
            ServerRequest::PermissionsRequestApproval { params, .. } => {
                // TODO(anp): Remove this native-path localization error path once core permission
                // paths remain PathUri after crossing the app-server boundary.
                match request_permissions_from_params(params) {
                    Ok(event) => self.on_request_permissions(event),
                    Err(err) => {
                        self.add_error_message(format!(
                            "failed to localize requested filesystem paths: {err}"
                        ));
                    }
                }
            }
            ServerRequest::ToolRequestUserInput { params, .. } => {
                self.on_request_user_input(params);
            }
            ServerRequest::DynamicToolCall { .. }
            | ServerRequest::AttestationGenerate { .. }
            | ServerRequest::CurrentTimeRead { .. }
            | ServerRequest::BrowserOpen { .. }
            | ServerRequest::ApplyPatchApproval { .. }
            | ServerRequest::ExecCommandApproval { .. } => {
                if replay_kind.is_none() {
                    self.add_error_message(TUI_STUB_MESSAGE.to_string());
                }
            }
        }
    }

    pub(crate) fn handle_skills_list_response(&mut self, response: SkillsListResponse) {
        self.on_list_skills(response);
    }

    pub(super) fn on_patch_apply_output_delta(&mut self, _item_id: String, _delta: String) {}

    pub(super) fn on_guardian_review_notification(
        &mut self,
        id: String,
        turn_id: String,
        started_at_ms: i64,
        review: forall_app_server_protocol::GuardianApprovalReview,
        completion: Option<(i64, forall_app_server_protocol::AutoReviewDecisionSource)>,
        action: GuardianApprovalReviewAction,
    ) {
        // TODO(anp): Remove this native-path localization error path once core permission paths
        // remain PathUri after crossing the app-server boundary.
        let action = match action.try_into() {
            Ok(action) => action,
            Err(err) => {
                self.add_error_message(format!(
                    "failed to localize guardian filesystem paths: {err}"
                ));
                return;
            }
        };
        let (completed_at_ms, decision_source) = match completion {
            Some((completed_at_ms, decision_source)) => {
                (Some(completed_at_ms), Some(decision_source))
            }
            None => (None, None),
        };

        self.on_guardian_assessment(GuardianAssessmentEvent {
            id,
            target_item_id: None,
            turn_id,
            started_at_ms,
            completed_at_ms,
            status: match review.status {
                forall_app_server_protocol::GuardianApprovalReviewStatus::InProgress => {
                    GuardianAssessmentStatus::InProgress
                }
                forall_app_server_protocol::GuardianApprovalReviewStatus::Approved => {
                    GuardianAssessmentStatus::Approved
                }
                forall_app_server_protocol::GuardianApprovalReviewStatus::Denied => {
                    GuardianAssessmentStatus::Denied
                }
                forall_app_server_protocol::GuardianApprovalReviewStatus::TimedOut => {
                    GuardianAssessmentStatus::TimedOut
                }
                forall_app_server_protocol::GuardianApprovalReviewStatus::Aborted => {
                    GuardianAssessmentStatus::Aborted
                }
            },
            risk_level: review.risk_level.map(|risk_level| match risk_level {
                forall_app_server_protocol::GuardianRiskLevel::Low => {
                    forall_protocol::approvals::GuardianRiskLevel::Low
                }
                forall_app_server_protocol::GuardianRiskLevel::Medium => {
                    forall_protocol::approvals::GuardianRiskLevel::Medium
                }
                forall_app_server_protocol::GuardianRiskLevel::High => {
                    forall_protocol::approvals::GuardianRiskLevel::High
                }
                forall_app_server_protocol::GuardianRiskLevel::Critical => {
                    forall_protocol::approvals::GuardianRiskLevel::Critical
                }
            }),
            user_authorization: review.user_authorization.map(|user_authorization| {
                match user_authorization {
                    forall_app_server_protocol::GuardianUserAuthorization::Unknown => {
                        forall_protocol::approvals::GuardianUserAuthorization::Unknown
                    }
                    forall_app_server_protocol::GuardianUserAuthorization::Low => {
                        forall_protocol::approvals::GuardianUserAuthorization::Low
                    }
                    forall_app_server_protocol::GuardianUserAuthorization::Medium => {
                        forall_protocol::approvals::GuardianUserAuthorization::Medium
                    }
                    forall_app_server_protocol::GuardianUserAuthorization::High => {
                        forall_protocol::approvals::GuardianUserAuthorization::High
                    }
                }
            }),
            rationale: review.rationale,
            decision_source: decision_source.map(|source| match source {
                forall_app_server_protocol::AutoReviewDecisionSource::Agent => {
                    GuardianAssessmentDecisionSource::Agent
                }
            }),
            action,
        });
    }

    pub(super) fn on_shutdown_complete(&mut self) {
        self.request_immediate_exit();
    }

    pub(super) fn on_turn_diff(&mut self, unified_diff: String) {
        debug!("TurnDiffEvent: {unified_diff}");
        self.refresh_status_line();
    }

    pub(super) fn on_deprecation_notice(&mut self, summary: String, details: Option<String>) {
        self.add_to_history(history_cell::new_deprecation_notice(summary, details));
        self.request_redraw();
    }
}
