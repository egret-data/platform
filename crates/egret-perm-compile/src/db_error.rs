//! The other crates' error types as a [`CompileError`]. Each one that carries
//! a `sqlx::Error` hands it to [`CompileError::from_db`], the one place a
//! database error's SQLSTATE is read, so a lock timeout (55P03) stays
//! [`CompileError::LockTimeout`] whatever language the server's message is in.

use egret_perm::CompileError;

/// A policy-store error.
pub fn from_store(e: egret_policy::PolicyStoreError) -> CompileError {
    match e {
        egret_policy::PolicyStoreError::Db(e) => CompileError::from_db(e),
        other => CompileError::Db(other.to_string()),
    }
}

/// An `egret_core` error, whose `Internal` arm carries the `sqlx::Error` of a
/// failed query (`egret_auth`'s repositories).
pub fn from_core(e: egret_core::Error) -> CompileError {
    match e {
        egret_core::Error::Internal(a) => match a.downcast::<sqlx::Error>() {
            Ok(e) => CompileError::from_db(e),
            Err(a) => CompileError::Db(egret_core::Error::Internal(a).to_string()),
        },
        other => CompileError::Db(other.to_string()),
    }
}

/// A lineage read's error.
pub fn from_lineage(e: crate::lineage::LineageError) -> CompileError {
    match e {
        crate::lineage::LineageError::Db(e) => CompileError::from_db(e),
        other => CompileError::Db(other.to_string()),
    }
}
