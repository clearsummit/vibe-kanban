//! HTTP routes for managing multi-account Claude OAuth.
//!
//! See `.specify/specs/claude-multi-account/contracts/*.openapi.yaml` for the wire shape.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::Json as ResponseJson,
    routing::{get, post},
};
use deployment::Deployment;
use serde::Deserialize;
use services::services::claude_accounts::{
    oauth::{ClaudeOAuthCompleteRequest, ClaudeOAuthStartRequest, ClaudeOAuthStartResponse},
    types::{ClaudeAccountView, ClaudeRetryPolicy},
};
use ts_rs::TS;
use utils::response::ApiResponse;
use uuid::Uuid;

use crate::{DeploymentImpl, error::ApiError};

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct UpdateClaudeAccount {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub disabled: Option<bool>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ReorderClaudeAccounts {
    /// Full desired ordering by account id. **The first id is picked
    /// first** by the rotator. The backend assigns precedence by
    /// position (`order[0]` → precedence 0, `order[1]` → precedence 1,
    /// …); lower numeric precedence = higher priority. Ids not present
    /// in this list keep their existing relative order at the end —
    /// defensive against stale clients that miss a newly-enrolled
    /// account.
    pub order: Vec<Uuid>,
}

async fn list_accounts(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<Vec<ClaudeAccountView>>>, ApiError> {
    let views = deployment.claude_accounts().store.list_views().await;
    Ok(ResponseJson(ApiResponse::success(views)))
}

async fn oauth_start(
    State(deployment): State<DeploymentImpl>,
    // The body is OPTIONAL per the OpenAPI contract — a client enrolling
    // a fresh account doesn't need to send anything. Only re-auth flows
    // include `account_id`. Default to None when the request has no body
    // or `{}`.
    payload: Option<Json<ClaudeOAuthStartRequest>>,
) -> Result<ResponseJson<ApiResponse<ClaudeOAuthStartResponse>>, ApiError> {
    let account_id = payload.and_then(|Json(p)| p.account_id);
    let resp = deployment.claude_accounts().oauth.start(account_id);
    Ok(ResponseJson(ApiResponse::success(resp)))
}

async fn oauth_complete(
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<ClaudeOAuthCompleteRequest>,
) -> Result<ResponseJson<ApiResponse<ClaudeAccountView>>, ApiError> {
    let svc = deployment.claude_accounts();
    let (creds, info, existing_id) = svc
        .oauth
        .complete(&payload.state, &payload.code)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let view = match existing_id {
        Some(id) => svc
            .store
            .replace_credentials(id, creds, info.as_ref())
            .await
            .map_err(store_err)?,
        None => svc
            .store
            .add(creds, info.as_ref())
            .await
            .map_err(store_err)?,
    };
    Ok(ResponseJson(ApiResponse::success(view)))
}

async fn patch_account(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateClaudeAccount>,
) -> Result<ResponseJson<ApiResponse<ClaudeAccountView>>, ApiError> {
    let store = &deployment.claude_accounts().store;
    let mut view: Option<ClaudeAccountView> = None;
    if let Some(label) = payload.label {
        view = Some(store.update_label(id, label).await.map_err(store_err)?);
    }
    if let Some(disabled) = payload.disabled {
        view = Some(store.set_disabled(id, disabled).await.map_err(store_err)?);
    }
    let view = match view {
        Some(v) => v,
        None => store
            .list_views()
            .await
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| ApiError::NotFound("account not found".into()))?,
    };
    Ok(ResponseJson(ApiResponse::success(view)))
}

async fn delete_account(
    State(deployment): State<DeploymentImpl>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    deployment
        .claude_accounts()
        .store
        .remove(id)
        .await
        .map_err(store_err)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reorder_accounts(
    State(deployment): State<DeploymentImpl>,
    Json(payload): Json<ReorderClaudeAccounts>,
) -> Result<ResponseJson<ApiResponse<Vec<ClaudeAccountView>>>, ApiError> {
    let views = deployment
        .claude_accounts()
        .store
        .reorder(&payload.order)
        .await
        .map_err(store_err)?;
    Ok(ResponseJson(ApiResponse::success(views)))
}

async fn get_retry_policy(
    State(deployment): State<DeploymentImpl>,
) -> Result<ResponseJson<ApiResponse<ClaudeRetryPolicy>>, ApiError> {
    let policy = deployment.config().read().await.claude_retry_policy;
    Ok(ResponseJson(ApiResponse::success(policy)))
}

async fn put_retry_policy(
    State(deployment): State<DeploymentImpl>,
    Json(policy): Json<ClaudeRetryPolicy>,
) -> Result<ResponseJson<ApiResponse<ClaudeRetryPolicy>>, ApiError> {
    policy
        .validate()
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    {
        let mut cfg = deployment.config().write().await;
        cfg.claude_retry_policy = policy;
    }
    let cfg = deployment.config().read().await.clone();
    services::services::config::save_config_to_file(&cfg, &utils::assets::config_path()).await?;
    Ok(ResponseJson(ApiResponse::success(policy)))
}

fn store_err(err: services::services::claude_accounts::store::StoreError) -> ApiError {
    use services::services::claude_accounts::store::StoreError;
    match err {
        // Aligns with the OpenAPI contract — 404 (not 400) for missing ids.
        StoreError::NotFound(id) => ApiError::NotFound(format!("account {id} not found")),
        StoreError::Io(e) => ApiError::Io(e),
        StoreError::Json(e) => ApiError::BadRequest(format!("invalid stored data: {e}")),
        StoreError::OAuth(e) => ApiError::BadRequest(e.to_string()),
    }
}

pub fn router() -> Router<DeploymentImpl> {
    Router::new().nest(
        "/claude-accounts",
        Router::new()
            .route("/", get(list_accounts))
            .route("/oauth/start", post(oauth_start))
            .route("/oauth/complete", post(oauth_complete))
            .route("/reorder", post(reorder_accounts))
            .route("/retry-policy", get(get_retry_policy).put(put_retry_policy))
            .route(
                "/{id}",
                axum::routing::patch(patch_account).delete(delete_account),
            ),
    )
}
