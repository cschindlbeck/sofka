use super::*;
use http_body_util::BodyExt;

fn helmrelease(name: &str) -> Value {
    json!({
        "apiVersion": "helm.toolkit.fluxcd.io/v2",
        "kind": "HelmRelease",
        "metadata": {"name": name, "namespace": "default"}
    })
}

fn choose(app: &mut App, item: &str) {
    app.handle_key(press(KeyCode::Char('t'))).unwrap();
    assert_eq!(app.mode, Mode::FluxMenu);
    let index = app
        .action_menu_items()
        .iter()
        .position(|s| *s == item)
        .unwrap();
    for _ in 0..index {
        app.handle_key(press(KeyCode::Char('j'))).unwrap();
    }
    app.handle_key(press(KeyCode::Enter)).unwrap();
}

#[tokio::test]
async fn helmrelease_reconcile_patches_selected_and_marked_releases() {
    for (force, bulk, fail) in [
        (false, false, false),
        (true, false, false),
        (true, true, false),
        (true, false, true),
    ] {
        let (mut app, mut rx) = test_app();
        app.switch_kind("helmreleases");
        apply(&mut app, helmrelease("apps"));
        apply(&mut app, helmrelease("infra"));
        let (requests, mut received) = mpsc::unbounded_channel();
        app.cluster.client = kube::Client::new(
            tower::service_fn(move |request: http::Request<kube::client::Body>| {
                let requests = requests.clone();
                async move {
                    let (parts, body) = request.into_parts();
                    assert_eq!(parts.method, http::Method::PATCH);
                    assert_eq!(
                        parts.headers["content-type"],
                        "application/merge-patch+json"
                    );
                    let bytes = body.collect().await.unwrap().to_bytes();
                    let patch: Value = serde_json::from_slice(&bytes).unwrap();
                    requests
                        .send((parts.uri.path().to_string(), patch))
                        .unwrap();
                    let (status, body) = if fail {
                        (
                            403,
                            json!({
                                "apiVersion": "v1", "kind": "Status", "status": "Failure",
                                "reason": "Forbidden", "message": "patch denied", "code": 403
                            }),
                        )
                    } else {
                        (200, helmrelease("apps"))
                    };
                    Ok::<_, std::convert::Infallible>(
                        http::Response::builder()
                            .status(status)
                            .body(http_body_util::Full::new(hyper::body::Bytes::from(
                                body.to_string(),
                            )))
                            .unwrap(),
                    )
                }
            }),
            "default",
        );
        if bulk {
            app.handle_key(press(KeyCode::Char(' '))).unwrap();
            app.handle_key(press(KeyCode::Char(' '))).unwrap();
            assert_eq!(app.marked.len(), 2);
        }
        let action = if force {
            "force reconcile"
        } else {
            "reconcile"
        };
        choose(
            &mut app,
            if force {
                "Force reconcile"
            } else {
                "Reconcile now"
            },
        );
        assert_eq!(app.mode, Mode::Table);
        assert!(app.marked.is_empty());
        let mut paths = Vec::new();
        for _ in 0..if bulk { 2 } else { 1 } {
            let (path, patch) = tokio::time::timeout(Duration::from_secs(2), received.recv())
                .await
                .unwrap()
                .unwrap();
            paths.push(path);
            let annotations = &patch["metadata"]["annotations"];
            let requested = annotations["reconcile.fluxcd.io/requestedAt"]
                .as_str()
                .unwrap();
            assert!(requested.parse::<Timestamp>().is_ok());
            assert_eq!(
                annotations.as_object().unwrap().len(),
                if force { 2 } else { 1 }
            );
            if force {
                assert_eq!(annotations["reconcile.fluxcd.io/forceAt"], requested);
            } else {
                assert!(annotations.get("reconcile.fluxcd.io/forceAt").is_none());
            }
            assert!(patch.get("spec").is_none());
        }
        paths.sort();
        let kind = app.kind.as_ref().unwrap();
        let prefix = format!(
            "/apis/helm.toolkit.fluxcd.io/{}/namespaces/default/helmreleases",
            kind.ar.version
        );
        let expected = if bulk {
            vec![format!("{prefix}/apps"), format!("{prefix}/infra")]
        } else {
            vec![format!("{prefix}/apps")]
        };
        assert_eq!(paths, expected);
        let reply = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let msg = rx.recv().await.unwrap();
                if matches!(&msg, Msg::Flash { message, .. } if message.starts_with(action)) {
                    break msg;
                }
            }
        })
        .await
        .unwrap();
        app.handle_msg(reply);
        assert_eq!(app.flash_err, fail);
        if fail {
            assert!(app.flash.starts_with("force reconcile apps failed:"));
        } else {
            let target = if bulk { "2 helmreleases" } else { "apps" };
            assert_eq!(app.flash, format!("{action} requested: {target}"));
        }
        assert!(received.try_recv().is_err());
    }
}

#[tokio::test]
async fn force_reconcile_menu_is_limited_to_flux_helmreleases() {
    for (plural, group, kind) in [
        ("helmreleases", "helm.toolkit.fluxcd.io", "HelmRelease"),
        ("helmreleases", "example.com", "HelmRelease"),
        (
            "kustomizations",
            "kustomize.toolkit.fluxcd.io",
            "Kustomization",
        ),
        (
            "gitrepositories",
            "source.toolkit.fluxcd.io",
            "GitRepository",
        ),
        ("cronjobs", "batch", "CronJob"),
        ("applications", "argoproj.io", "Application"),
    ] {
        let (mut app, _rx) = test_app();
        app.cluster.register_kind(group, kind, plural, true);
        app.switch_kind(plural);
        apply(
            &mut app,
            json!({
                "apiVersion": format!("{group}/v1"), "kind": kind,
                "metadata": {"name": "apps", "namespace": "default"}
            }),
        );
        app.handle_key(press(KeyCode::Char('t'))).unwrap();
        assert_eq!(app.mode, Mode::FluxMenu);
        assert_eq!(
            app.action_menu_items().contains(&"Force reconcile"),
            group == "helm.toolkit.fluxcd.io"
        );
    }
}

#[tokio::test]
async fn helmrelease_menu_cancel_and_readonly_do_not_start_an_action() {
    for cancel in [KeyCode::Esc, KeyCode::Enter] {
        let (mut app, _rx) = test_app();
        app.switch_kind("helmreleases");
        apply(&mut app, helmrelease("apps"));
        app.handle_key(press(KeyCode::Char(' '))).unwrap();
        let flash = app.flash.clone();
        if cancel == KeyCode::Enter {
            choose(&mut app, "Cancel");
        } else {
            app.handle_key(press(KeyCode::Char('t'))).unwrap();
            for _ in 0..3 {
                app.handle_key(press(KeyCode::Char('j'))).unwrap();
            }
            app.handle_key(press(cancel)).unwrap();
        }
        assert_eq!(app.mode, Mode::Table);
        assert_eq!(app.flash, flash);
        assert_eq!(app.marked.len(), 1);
        app.readonly = true;
        app.handle_key(press(KeyCode::Char('t'))).unwrap();
        assert_eq!(app.mode, Mode::Table);
        assert!(app.flash_err);
        assert_eq!(app.marked.len(), 1);
    }
}
