//! The HTTP client core shared by `io.kestra.plugin.core.http.Request`,
//! `Download` and `SseRequest` (package #4, spec D3–D6): Kestra's
//! `io.kestra.core.http.client` and `plugin.core.http.AbstractHttp`, on top of
//! the egress guard in `egret-egress`.
//!
//! - [`spec`]: the task properties as Kestra names them (`uri`, `method`,
//!   `body`, `contentType`, `formData`, `params`, `headers`, `options.*`), and
//!   [`refusals`], the D4 refusals (`options.proxy`,
//!   `options.ssl.insecureTrustAllCertificates`, `options.auth.type: DIGEST`,
//!   `encryptBody`) used both as a publish finding and at run time.
//! - [`request`]: the request builder (`AbstractHttp.request` and the auth
//!   interceptors), and [`HttpEgress`], the deployment's policy from
//!   `workflow.http`.
//! - [`response`]: the status rule (`HttpClient.isAllowedStatusCode`), the
//!   capped body read, the failure message (at most 4 KiB of body, masked),
//!   and the `headers` output.
//! - [`sse`]: the Server-Sent Events parser of `SseRequest`.
//! - [`mask`]: masking of resolved secrets and credentials, and the
//!   `options.logs` lines with credential headers never printed.
//!
//! Nothing here renders: the handler renders the task's properties (resolving
//! `secret()`, D5) and hands the rendered map to [`HttpSpec::from_value`].
//! [`refusals`] also accepts the unrendered map, which is what publish has.

pub mod mask;
pub mod request;
pub mod response;
pub mod spec;
pub mod sse;

#[cfg(test)]
mod tests;

pub use mask::{log_request_lines, log_response_lines, MaskSet};
pub use request::{build_request, BuiltRequest, FormFiles, HttpEgress};
pub use response::{
    check_status, ensure_allowed, failure_message, headers_output, read_capped, read_prefix,
    transport_message, BodyTooLarge, ReadError, FAILURE_BODY_BYTES,
};
pub use spec::{refusals, HttpSpec, LogKind, ResolvedOptions};
