//! User-facing product names for the Forall agent TUI.

macro_rules! define_branding {
    ($company:literal, $product:literal) => {
        pub const COMPANY_NAME: &str = $company;
        pub const PRODUCT_NAME: &str = $product;
        pub const PRODUCT_FULL_NAME: &str = concat!($company, " ", $product);
        pub const PLACEHOLDER_PROMPT: &str = concat!("Ask ", $product, " to do anything");
    };
}

define_branding!("Astrio", "Forall");

/// Static ∀ mark for onboarding (`frames/forall/frame.txt`).
pub const ONBOARDING_SYMBOL_ASCII_ART: &str = include_str!("../frames/forall/frame.txt");

/// ASCII art shown at the top of the session header.
///
/// Paste your art here — each line becomes one row in the TUI.
pub const SESSION_HEADER_ASCII_ART: &str = "
███████  ██████  ██████   █████  ██      ██      
██      ██    ██ ██   ██ ██   ██ ██      ██      
█████   ██    ██ ██████  ███████ ██      ██      
██      ██    ██ ██   ██ ██   ██ ██      ██      
██       ██████  ██   ██ ██   ██ ███████ ███████                                          
";

pub(crate) fn welcome_tip(suffix: &str) -> String {
    format!("*Tip* Welcome to {COMPANY_NAME} {PRODUCT_NAME} — {suffix}")
}
