use super::CheckStatus;
use super::DoctorCheck;

pub(super) fn updates_check() -> DoctorCheck {
    DoctorCheck::new(
        "updates.status",
        "updates",
        CheckStatus::Ok,
        "Source build. Update the checkout and rebuild the CLI to upgrade.",
    )
}
