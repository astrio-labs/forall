//! Settings-adjacent popup surfaces for `ChatWidget`.
//!
//! This keeps theme and experimental-feature UI out of the main orchestration
//! module without changing their event wiring.

use super::*;

impl ChatWidget {
    pub(super) fn open_theme_picker(&mut self) {
        let forall_home = forall_utils_home_dir::find_forall_home().ok();
        let terminal_width = self
            .last_rendered_width
            .get()
            .and_then(|width| u16::try_from(width).ok());
        let params = crate::theme_picker::build_theme_picker_params(
            self.config.tui_theme.as_deref(),
            forall_home.as_deref(),
            terminal_width,
        );
        self.bottom_pane.show_selection_view(params);
    }

    pub(crate) fn open_experimental_popup(&mut self) {
        let features: Vec<ExperimentalFeatureItem> = FEATURES
            .iter()
            .filter_map(|spec| {
                let name = spec.stage.experimental_menu_name()?;
                let description = spec.stage.experimental_menu_description()?;
                Some(ExperimentalFeatureItem {
                    feature: spec.id,
                    name: name.to_string(),
                    description: description.to_string(),
                    enabled: self.config.features.enabled(spec.id),
                })
            })
            .collect();

        let view = ExperimentalFeaturesView::new(
            features,
            self.app_event_tx.clone(),
            self.bottom_pane.list_keymap(),
        );
        self.bottom_pane.show_view(Box::new(view));
    }
}
