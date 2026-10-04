//! `DataApiEndpointPort` implementation over the concrete [`EndpointStore`].
//!
//! This is the dataapi-side of the fix-round-2 seam: `egret-automation` holds
//! `Arc<dyn egret_dataapi_api::DataApiEndpointPort>`, and `egret-bin` hands it
//! an `Arc<EndpointStore>` via this impl.
//!
//! `self.workspace_of(id)` inside the trait method below calls the INHERENT
//! method of the same name on `EndpointStore` (`endpoints.rs`), not this
//! trait method recursively — Rust resolves inherent methods before trait
//! methods, so this is a plain delegation, not infinite recursion.

use uuid::Uuid;

use crate::endpoints::EndpointStore;

#[async_trait::async_trait]
impl egret_dataapi_api::DataApiEndpointPort for EndpointStore {
    async fn workspace_of(&self, id: Uuid) -> Result<Option<Uuid>, egret_core::Error> {
        self.workspace_of(id).await
    }
}
