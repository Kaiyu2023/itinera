use axum::{Json, extract::State};
use itinera_core::{
    domain::user::User, ports::authorization::TripAuthorizationContext, services::trips,
};
use serde::Serialize;

use crate::{auth::AuthenticatedPrincipal, error::ApiError, state::AppState};

/// The palette the frontend already renders member avatars with
/// (`frontend/src/api/mock/fixtures/users.ts`).
const AVATAR_PALETTE: [&str; 6] = [
    "#6b5bd2", "#a0522d", "#e6b422", "#e05263", "#3b6fd4", "#4fb06d",
];

/// The wire shape of `User` (`openapi.yaml` → `components.schemas.User`).
///
/// All four fields are required and non-nullable, but the domain carries only
/// two of them: `display_name` is optional and nothing sets it yet, and colour
/// is not a domain concept at all. Both are therefore derived here, which is
/// temporary — once the contract grows a `PATCH /me` they become stored fields
/// the user chooses, and this derivation survives only as the seed value.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserResponse {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub avatar_color: String,
}

impl From<User> for UserResponse {
    fn from(user: User) -> Self {
        let email = user.email.to_string();
        let avatar_color = avatar_color(&email).to_string();
        let display_name = user
            .display_name
            .unwrap_or_else(|| local_part(&email).to_string());

        UserResponse {
            id: user.id.0,
            email,
            display_name,
            avatar_color,
        }
    }
}

pub async fn get_me(
    principal: AuthenticatedPrincipal,
    State(state): State<AppState>,
) -> Result<Json<UserResponse>, ApiError> {
    let user = principal.require_human()?;
    let authorization = TripAuthorizationContext::human(user.id.clone());
    // The application calls /me on entry, making this the deterministic point
    // where a pending email invite becomes authoritative membership. The
    // lookup is strongly consistent and trip-scoped writes remain atomic.
    trips::accept_pending_invites(&*state.trips, &authorization, &user, &state.clock.now()).await?;
    Ok(Json(user.into()))
}

/// Everything before the `@`. `Email::parse` guarantees the separator exists,
/// so the fallback arm is unreachable — it is there to keep this total rather
/// than to handle a real case.
fn local_part(email: &str) -> &str {
    email.split_once('@').map_or(email, |(local, _)| local)
}

/// Deterministic pick from [`AVATAR_PALETTE`], so a user keeps their colour
/// across restarts without anything being stored.
///
/// The fold is written out rather than delegated to `DefaultHasher`, whose
/// output is explicitly not stable across Rust releases — a toolchain upgrade
/// would silently reshuffle every avatar. Multiplying before adding makes the
/// result order-sensitive, so `ann@x` and `nna@x` do not collide.
fn avatar_color(email: &str) -> &'static str {
    let fold = email.bytes().fold(0u32, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte.into())
    });
    AVATAR_PALETTE[fold as usize % AVATAR_PALETTE.len()]
}

#[cfg(test)]
mod tests {
    use itinera_core::domain::user::{Email, UserId};

    use super::*;

    fn user(email: &str, display_name: Option<&str>) -> User {
        User {
            id: UserId("u-test".to_string()),
            email: Email::parse(email).expect("fixture email should parse"),
            display_name: display_name.map(str::to_string),
        }
    }

    #[test]
    fn a_missing_display_name_falls_back_to_the_email_local_part() {
        let response = UserResponse::from(user("cloud.strife@proton.me", None));

        assert_eq!(response.display_name, "cloud.strife");
    }

    #[test]
    fn a_stored_display_name_wins_over_the_fallback() {
        let response = UserResponse::from(user("cloud.strife@proton.me", Some("Cloud")));

        assert_eq!(response.display_name, "Cloud");
    }

    #[test]
    fn the_email_is_carried_in_its_canonical_form() {
        let response = UserResponse::from(user("  Cloud.Strife@Proton.ME  ", None));

        assert_eq!(response.email, "cloud.strife@proton.me");
        assert_eq!(
            response.display_name, "cloud.strife",
            "the fallback derives from the canonical address, not the raw input"
        );
    }

    #[test]
    fn the_avatar_colour_is_always_from_the_palette() {
        // Swept across several addresses because the failure mode would be an
        // index out of range for one particular fold, not a systematic one.
        for local in ["a", "bb", "ccc", "cloud.strife", "z9", "ann", "nna"] {
            let email = format!("{local}@proton.me");
            let response = UserResponse::from(user(&email, None));

            assert!(
                AVATAR_PALETTE.contains(&response.avatar_color.as_str()),
                "{email} produced {}, which is not in the palette",
                response.avatar_color
            );
        }
    }

    #[test]
    fn the_same_address_always_gets_the_same_colour() {
        // The property that lets us derive the colour rather than store it.
        let first = UserResponse::from(user("cloud.strife@proton.me", None));
        let second = UserResponse::from(user("cloud.strife@proton.me", Some("Cloud")));

        assert_eq!(first.avatar_color, second.avatar_color);
    }

    #[test]
    fn the_colour_depends_on_the_order_of_the_characters() {
        // A plain byte sum would collide here; that is why the fold multiplies.
        let ann = UserResponse::from(user("ann@proton.me", None));
        let nna = UserResponse::from(user("nna@proton.me", None));

        assert_ne!(ann.avatar_color, nna.avatar_color);
    }

    #[test]
    fn serialises_with_the_field_names_the_contract_requires() {
        let response = UserResponse::from(user("cloud.strife@proton.me", None));

        let json = serde_json::to_value(&response).expect("should serialise");
        let object = json.as_object().expect("should be a JSON object");

        // `serde_json` may use either a sorted map or insertion order depending
        // on dependency feature unification. The wire contract is the set of
        // names, not the object's serialization order.
        let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["avatarColor", "displayName", "email", "id"],
            "openapi.yaml requires exactly these four keys, camelCased"
        );
    }
}
