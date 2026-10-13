//! Default-on delegation resolution: an omitted engine `delegationMode` resolves
//! to per-user JWT (trust egret's machine-IdP), degrading to `impersonate` when
//! JWT can't be guaranteed. See docs/en/specs/2026-07-21-engine-jwt-delegation-default-design.md.

/// The concrete delegation outcome after applying the default-resolution rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedDelegation {
    /// Per-user JWT delegation (engine trusts egret's machine-IdP).
    Jwt,
    /// Service-user impersonation (`EXECUTE AS` / `X-Trino-User`).
    Impersonate,
    /// No delegation — an "open" service-user engine (the explicit `"none"` opt-out).
    Open,
}

/// True iff the platform can mint/validate engine JWTs — egret's machine-IdP JWKS
/// URI and issuer are both set (⟺ `catalog.public_url` is configured).
pub fn has_platform_jwks(engine_jwks_uri: &str, engine_oidc_issuer: &str) -> bool {
    !engine_jwks_uri.is_empty() && !engine_oidc_issuer.is_empty()
}

/// Best-effort: does a StarRocks FE image tag CLEARLY predate `authentication_jwt`
/// (added in 3.5)? Parses the tag after the last `:` as `major.minor…`. Returns
/// `true` ONLY when it clearly parses to `< 3.5`; an absent/unparseable tag
/// (`latest`, a digest, empty, no tag) returns `false` — assume capable, the
/// default image is 4.1. This only steers the DEFAULT toward `impersonate`; it is
/// never a hard-fail.
pub fn starrocks_image_is_pre_3_5(image: Option<&str>) -> bool {
    let Some(image) = image else { return false };
    // A pinned ref is `repo:tag@sha256:<hex>`; the digest's own `:` is not the tag's.
    let image = image.split('@').next().unwrap_or(image);
    let tag = match image.rsplit_once(':') {
        Some((_, t)) => t,
        None => return false,
    };
    let mut parts = tag.trim_start_matches('v').split('.');
    let (Some(maj), Some(min)) = (parts.next(), parts.next()) else {
        return false;
    };
    let (Ok(maj), Ok(min)) = (maj.parse::<u32>(), min.parse::<u32>()) else {
        return false;
    };
    maj < 3 || (maj == 3 && min < 5)
}

/// Resolve the effective delegation mode.
///
/// - `explicit`: the raw wire `delegationMode` (`"jwt"|"impersonate"|"none"`), or
///   `None`/`""` when omitted.
/// - `has_jwks`: [`has_platform_jwks`].
/// - `sr_image`: the StarRocks FE image tag; `None` for Trino (no version floor).
///
/// Returns `Err` (→ a 400 message) ONLY for an explicit `"jwt"` the platform can't
/// satisfy, or an unrecognized value. The DEFAULT path (omitted) never errors — it
/// degrades to `Impersonate`.
pub fn resolve_delegation(
    explicit: Option<&str>,
    has_jwks: bool,
    sr_image: Option<&str>,
) -> Result<ResolvedDelegation, String> {
    match explicit.map(str::trim).filter(|s| !s.is_empty()) {
        Some("none") => Ok(ResolvedDelegation::Open),
        Some("impersonate") => Ok(ResolvedDelegation::Impersonate),
        Some("jwt") if has_jwks => Ok(ResolvedDelegation::Jwt),
        Some("jwt") => Err(
            "JWT delegation was requested but this platform has no machine-IdP JWKS. \
             Set `catalog.public_url`, or pass delegationMode:\"none\"/\"impersonate\"."
                .to_string(),
        ),
        Some(other) => Err(format!(
            "invalid delegationMode: {other} (expected \"jwt\", \"impersonate\", or \"none\")"
        )),
        None => Ok(if has_jwks && !starrocks_image_is_pre_3_5(sr_image) {
            ResolvedDelegation::Jwt
        } else {
            ResolvedDelegation::Impersonate
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_pre_3_5_parse() {
        assert!(starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu:3.4")));
        assert!(starrocks_image_is_pre_3_5(Some(
            "starrocks/fe-ubuntu:3.4.10"
        )));
        assert!(starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu:2.5")));
        assert!(!starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu:3.5")));
        assert!(!starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu:4.1")));
        assert!(!starrocks_image_is_pre_3_5(Some(
            "starrocks/fe-ubuntu:latest"
        )));
        assert!(!starrocks_image_is_pre_3_5(Some("starrocks/fe-ubuntu")));
        assert!(!starrocks_image_is_pre_3_5(Some("")));
        let digest = "@sha256:d07aa750be77625a09866a76dbfe0ee032baeb5e48fe0e952d30cb525329dc31";
        assert!(starrocks_image_is_pre_3_5(Some(&format!(
            "starrocks/fe-ubuntu:3.4.10{digest}"
        ))));
        assert!(!starrocks_image_is_pre_3_5(Some(&format!(
            "starrocks/fe-ubuntu:4.1.6{digest}"
        ))));
        assert!(!starrocks_image_is_pre_3_5(None));
    }

    #[test]
    fn explicit_modes_are_honored() {
        assert_eq!(
            resolve_delegation(Some("none"), true, None),
            Ok(ResolvedDelegation::Open)
        );
        assert_eq!(
            resolve_delegation(Some("impersonate"), false, None),
            Ok(ResolvedDelegation::Impersonate)
        );
        assert_eq!(
            resolve_delegation(Some("jwt"), true, None),
            Ok(ResolvedDelegation::Jwt)
        );
    }

    #[test]
    fn explicit_jwt_without_jwks_is_rejected() {
        let err = resolve_delegation(Some("jwt"), false, None).unwrap_err();
        assert!(err.contains("public_url"), "message names the fix: {err}");
    }

    #[test]
    fn unknown_mode_is_rejected() {
        assert!(resolve_delegation(Some("bogus"), true, None).is_err());
    }

    #[test]
    fn default_prefers_jwt_then_degrades() {
        assert_eq!(
            resolve_delegation(None, true, Some("starrocks/fe-ubuntu:4.1")),
            Ok(ResolvedDelegation::Jwt)
        );
        assert_eq!(
            resolve_delegation(None, true, None),
            Ok(ResolvedDelegation::Jwt)
        );
        assert_eq!(
            resolve_delegation(None, true, Some("starrocks/fe-ubuntu:3.4")),
            Ok(ResolvedDelegation::Impersonate)
        );
        assert_eq!(
            resolve_delegation(None, false, None),
            Ok(ResolvedDelegation::Impersonate)
        );
        assert_eq!(
            resolve_delegation(Some(""), false, None),
            Ok(ResolvedDelegation::Impersonate)
        );
    }

    #[test]
    fn has_platform_jwks_needs_both() {
        assert!(has_platform_jwks("https://e/jwks", "https://e"));
        assert!(!has_platform_jwks("", "https://e"));
        assert!(!has_platform_jwks("https://e/jwks", ""));
    }
}
