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
        me::get_me,
        plans::{get_current_plan, initialize_plan, list_plan_versions, update_day, update_stop},
        polls::{
            POLL_BODYLESS_LIMIT_BYTES, POLL_CREATE_BODY_LIMIT_BYTES, POLL_VOTE_BODY_LIMIT_BYTES,
            close_poll, create_poll, list_polls, open_poll, vote,
        },
        proposals::{
            approve_proposal, create_proposal, list_proposals, proposal_to_poll, reject_proposal,
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
        .merge(content_history_routes)
        .with_state(state)
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}
