use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, patch, post},
};
use serde_json::json;

use crate::{
    routes::{
        candidates::{
            add_candidate, list_candidates, search_places, set_candidate_status, update_candidate,
        },
        content_history::{get_history, revert_edit},
        discussions::{
            DISCUSSION_BODYLESS_LIMIT_BYTES, DISCUSSION_REACTION_BODY_LIMIT_BYTES,
            DISCUSSION_WRITE_BODY_LIMIT_BYTES, add_comment, create_thread, get_comments,
            list_threads, set_reaction,
        },
        ledger::{
            LEDGER_BODYLESS_LIMIT_BYTES, LEDGER_WRITE_BODY_LIMIT_BYTES, add_expense,
            add_settlement, delete_expense, get_ledger, update_expense,
        },
        me::get_me,
        notices::{
            NOTICE_BODYLESS_LIMIT_BYTES, NOTICE_WRITE_BODY_LIMIT_BYTES, create_notice,
            list_notices, toggle_checklist_item, update_notice,
        },
        plans::{get_current_plan, initialize_plan, list_plan_versions, update_day, update_stop},
        polls::{
            POLL_BODYLESS_LIMIT_BYTES, POLL_CREATE_BODY_LIMIT_BYTES, POLL_VOTE_BODY_LIMIT_BYTES,
            close_poll, create_poll, list_polls, open_poll, vote,
        },
        proposals::{
            approve_proposal, create_proposal, list_proposals, proposal_to_poll, reject_proposal,
        },
        service_identities::{
            SERVICE_IDENTITY_BODYLESS_LIMIT_BYTES, SERVICE_IDENTITY_WRITE_LIMIT_BYTES,
            list_service_identities, register_service_identity, revoke_service_identity,
        },
        trips::{
            create_trip, get_trip, get_users, invite, list_trips, remove_member, set_trip_status,
        },
    },
    state::AppState,
};

pub mod auth;
pub mod error;
pub mod routes;
pub mod state;

pub fn create_app(state: AppState) -> Router {
    let content_history_routes = Router::new()
        .route("/trips/{tripId}/history", get(get_history))
        .route("/trips/{tripId}/edits/{editId}/revert", post(revert_edit))
        .layer(DefaultBodyLimit::max(
            routes::content_history::REVERT_BODY_LIMIT_BYTES,
        ));
    let bodyless_poll_routes = Router::new()
        .route(
            "/trips/{tripId}/proposals/{proposalId}/to-poll",
            post(proposal_to_poll),
        )
        .route("/trips/{tripId}/polls", get(list_polls))
        .route("/trips/{tripId}/polls/{pollId}/open", post(open_poll))
        .route("/trips/{tripId}/polls/{pollId}/close", post(close_poll))
        .layer(DefaultBodyLimit::max(POLL_BODYLESS_LIMIT_BYTES));
    let create_poll_routes = Router::new()
        .route("/trips/{tripId}/polls", post(create_poll))
        .layer(DefaultBodyLimit::max(POLL_CREATE_BODY_LIMIT_BYTES));
    let vote_routes = Router::new()
        .route("/trips/{tripId}/polls/{pollId}/votes", post(vote))
        .layer(DefaultBodyLimit::max(POLL_VOTE_BODY_LIMIT_BYTES));
    let bodyless_discussion_routes = Router::new()
        .route("/trips/{tripId}/threads", get(list_threads))
        .route(
            "/trips/{tripId}/threads/{threadId}/comments",
            get(get_comments),
        )
        .layer(DefaultBodyLimit::max(DISCUSSION_BODYLESS_LIMIT_BYTES));
    let discussion_write_routes = Router::new()
        .route("/trips/{tripId}/threads", post(create_thread))
        .route(
            "/trips/{tripId}/threads/{threadId}/comments",
            post(add_comment),
        )
        .layer(DefaultBodyLimit::max(DISCUSSION_WRITE_BODY_LIMIT_BYTES));
    let discussion_reaction_routes = Router::new()
        .route(
            "/trips/{tripId}/threads/{threadId}/comments/{commentId}/reactions",
            post(set_reaction),
        )
        .layer(DefaultBodyLimit::max(DISCUSSION_REACTION_BODY_LIMIT_BYTES));
    let ledger_bodyless_routes = Router::new()
        .route("/trips/{tripId}/ledger", get(get_ledger))
        .route(
            "/trips/{tripId}/expenses/{expenseId}",
            delete(delete_expense),
        )
        .layer(DefaultBodyLimit::max(LEDGER_BODYLESS_LIMIT_BYTES));
    let ledger_write_routes = Router::new()
        .route("/trips/{tripId}/expenses", post(add_expense))
        .route(
            "/trips/{tripId}/expenses/{expenseId}",
            patch(update_expense),
        )
        .route("/trips/{tripId}/settlements", post(add_settlement))
        .layer(DefaultBodyLimit::max(LEDGER_WRITE_BODY_LIMIT_BYTES));
    let notice_bodyless_routes = Router::new()
        .route("/trips/{tripId}/notices", get(list_notices))
        .route(
            "/trips/{tripId}/notices/{noticeId}/checklist/{itemId}/toggle",
            post(toggle_checklist_item),
        )
        .layer(DefaultBodyLimit::max(NOTICE_BODYLESS_LIMIT_BYTES));
    let notice_write_routes = Router::new()
        .route("/trips/{tripId}/notices", post(create_notice))
        .route("/trips/{tripId}/notices/{noticeId}", patch(update_notice))
        .layer(DefaultBodyLimit::max(NOTICE_WRITE_BODY_LIMIT_BYTES));
    let service_identity_bodyless_routes = Router::new()
        .route("/me/service-identities", get(list_service_identities))
        .route(
            "/me/service-identities/{serviceIdentityId}",
            delete(revoke_service_identity),
        )
        .layer(DefaultBodyLimit::max(SERVICE_IDENTITY_BODYLESS_LIMIT_BYTES));
    let service_identity_write_routes = Router::new()
        .route("/me/service-identities", post(register_service_identity))
        .layer(DefaultBodyLimit::max(SERVICE_IDENTITY_WRITE_LIMIT_BYTES));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/me", get(get_me))
        .route("/trips", get(list_trips))
        .route("/trips", post(create_trip))
        .route("/trips/{tripId}", get(get_trip))
        .route("/trips/{tripId}/status", patch(set_trip_status))
        .route("/trips/{tripId}/members", get(get_users))
        .route("/trips/{tripId}/members/{userId}", delete(remove_member))
        .route("/trips/{tripId}/invites", post(invite))
        .route("/trips/{tripId}/places/search", get(search_places))
        .route("/trips/{tripId}/candidates", get(list_candidates))
        .route("/trips/{tripId}/candidates", post(add_candidate))
        .route(
            "/trips/{tripId}/candidates/{candidateId}",
            patch(update_candidate),
        )
        .route(
            "/trips/{tripId}/candidates/{candidateId}/status",
            patch(set_candidate_status),
        )
        .route("/trips/{tripId}/plan", get(get_current_plan))
        .route("/trips/{tripId}/plan", post(initialize_plan))
        .route("/trips/{tripId}/plan/versions", get(list_plan_versions))
        .route("/trips/{tripId}/stops/{stopId}", patch(update_stop))
        .route("/trips/{tripId}/days/{dayId}", patch(update_day))
        .route("/trips/{tripId}/proposals", get(list_proposals))
        .route("/trips/{tripId}/proposals", post(create_proposal))
        .route(
            "/trips/{tripId}/proposals/{proposalId}/approve",
            post(approve_proposal),
        )
        .route(
            "/trips/{tripId}/proposals/{proposalId}/reject",
            post(reject_proposal),
        )
        .merge(bodyless_poll_routes)
        .merge(create_poll_routes)
        .merge(vote_routes)
        .merge(bodyless_discussion_routes)
        .merge(discussion_write_routes)
        .merge(discussion_reaction_routes)
        .merge(ledger_bodyless_routes)
        .merge(ledger_write_routes)
        .merge(notice_bodyless_routes)
        .merge(notice_write_routes)
        .merge(content_history_routes)
        .merge(service_identity_bodyless_routes)
        .merge(service_identity_write_routes)
        .with_state(state)
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}
