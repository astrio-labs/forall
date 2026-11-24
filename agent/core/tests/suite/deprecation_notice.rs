#![cfg(not(target_os = "windows"))]

use anyhow::Ok;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_forall::TestForall;
use core_test_support::test_forall::test_forall;
use core_test_support::wait_for_event_match;
use forall_features::Feature;
use forall_protocol::protocol::DeprecationNoticeEvent;
use forall_protocol::protocol::EventMsg;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn emits_deprecation_notice_for_legacy_feature_flag() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_forall().with_config(|config| {
        let mut features = config.features.get().clone();
        features.enable(Feature::UnifiedExec);
        features
            .record_legacy_usage_force("use_experimental_unified_exec_tool", Feature::UnifiedExec);
        config
            .features
            .set(features)
            .expect("test config should allow managed feature metadata updates");
        config.use_experimental_unified_exec_tool = true;
    });

    let TestForall { forall: codex, .. } = builder.build(&server).await?;

    let notice = wait_for_event_match(&codex, |event| match event {
        EventMsg::DeprecationNotice(ev) => Some(ev.clone()),
        _ => None,
    })
    .await;

    let DeprecationNoticeEvent { summary, details } = notice;
    assert_eq!(
        summary,
        "`[features].use_experimental_unified_exec_tool` is deprecated. Use `[features].unified_exec` instead.".to_string(),
    );
    assert_eq!(
        details.as_deref(),
        Some(
            "Enable it with `--enable unified_exec` or `[features].unified_exec` in config.toml. See https://github.com/astrio-labs/forall for configuration details."
        ),
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn emits_deprecation_notice_for_web_search_feature_flag_values() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    for enabled in [true, false] {
        let server = start_mock_server().await;

        let mut builder = test_forall().with_config(move |config| {
            let mut entries = BTreeMap::new();
            entries.insert("web_search_request".to_string(), enabled);
            let mut features = config.features.get().clone();
            features.apply_map(&entries);
            config
                .features
                .set(features)
                .expect("test config should allow managed feature map updates");
        });

        let TestForall { forall: codex, .. } = builder.build(&server).await?;

        let notice = wait_for_event_match(&codex, |event| match event {
            EventMsg::DeprecationNotice(ev)
                if ev.summary.contains("[features].web_search_request") =>
            {
                Some(ev.clone())
            }
            _ => None,
        })
        .await;

        let DeprecationNoticeEvent { summary, details } = notice;
        assert_eq!(
            summary,
            "`[features].web_search_request` is deprecated because web search is enabled by default."
                .to_string(),
        );
        assert_eq!(
            details.as_deref(),
            Some(
                "Set `web_search` to `\"live\"`, `\"indexed\"`, `\"cached\"`, or `\"disabled\"` at the top level (or under a profile) in config.toml if you want to override it."
            ),
        );
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn emits_deprecation_notice_for_use_legacy_landlock() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_forall().with_config(|config| {
        let mut entries = BTreeMap::new();
        entries.insert("use_legacy_landlock".to_string(), true);
        let mut features = config.features.get().clone();
        features.apply_map(&entries);
        config
            .features
            .set(features)
            .expect("test config should allow managed feature map updates");
    });

    let TestForall { forall: codex, .. } = builder.build(&server).await?;

    let notice = wait_for_event_match(&codex, |event| match event {
        EventMsg::DeprecationNotice(ev)
            if ev.summary.contains("[features].use_legacy_landlock") =>
        {
            Some(ev.clone())
        }
        _ => None,
    })
    .await;

    let DeprecationNoticeEvent { summary, details } = notice;
    assert_eq!(
        summary,
        "`[features].use_legacy_landlock` is deprecated and will be removed soon.".to_string(),
    );
    assert_eq!(
        details.as_deref(),
        Some("Remove this setting to stop opting into the legacy Linux sandbox behavior."),
    );

    Ok(())
}
