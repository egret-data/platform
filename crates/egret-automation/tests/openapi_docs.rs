//! The OpenAPI text of this crate's routes comes from their doc comments; a comment
//! left above the wrong handler documents the wrong route (KI-299).

fn get_op(path: &str) -> utoipa::openapi::path::Operation {
    let doc = egret_automation::openapi();
    let item = doc
        .paths
        .paths
        .get(path)
        .unwrap_or_else(|| panic!("{path} is not in the document"));
    item.get
        .clone()
        .unwrap_or_else(|| panic!("{path} has no GET"))
}

#[test]
fn the_slack_get_route_is_described_as_the_slack_app() {
    let op = get_op("/api/ws/{workspace}/notifications/slack");
    assert_eq!(op.summary.as_deref(), Some("The workspace's Slack app."));
    let description = op.description.unwrap_or_default();
    assert!(
        !description.contains("print page"),
        "the Slack route carries the render route's comment: {description}"
    );
}

#[test]
fn the_render_payload_route_keeps_its_own_comment() {
    let op = get_op("/api/render/payload");
    assert_eq!(op.summary.as_deref(), Some("The print page's ONE request."));
}
