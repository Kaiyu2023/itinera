use std::{
    fmt::Write,
    sync::{Arc, RwLock},
    time::Duration,
};

use itinera_core::{
    domain::{
        content_history::{EditEntity, EditStatus},
        poll::{Poll, PollKind, PollOption, PollStatus},
        proposal::{
            ChangeOp, ChangeSet, NewPlaceDraft, Proposal, ProposalDecision, ProposalRoute,
            ProposalStatus,
        },
        trip::{Candidate, CandidateStatus, Day, Place, PlaceKind, Plan, StopKind, TripRole},
        user::User,
    },
    ports::{
        authorization::TripAuthorizationContext,
        clock::Clock,
        content_history::{ContentHistoryRepo, ContentHistoryRepoError},
        id_gen::IdGen,
        poll::{NewDecisionPoll, NewPlanChangePoll, PollRepo, PollRepoError},
        proposal::{ProposalApplicationIds, ProposalRepo, ProposalRepoError},
        trip::{CandidateUpdate, TripRepo, TripRepoError},
    },
    services::polls::{CreatePollInput, create_poll},
};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Sqlite, Transaction};
use tokio::sync::Barrier;

use super::support::{NOW, TestDatabase, raw_connection, seed_trip, seed_user};

const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

struct FixedClock<'a>(&'a str);

impl Clock for FixedClock<'_> {
    fn now(&self) -> String {
        self.0.to_string()
    }
}

#[derive(Clone)]
struct MutableClock(Arc<RwLock<String>>);

impl MutableClock {
    fn new(value: &str) -> Self {
        Self(Arc::new(RwLock::new(value.to_string())))
    }

    fn set(&self, value: &str) {
        *self.0.write().expect("clock write lock") = value.to_string();
    }
}

impl Clock for MutableClock {
    fn now(&self) -> String {
        self.0.read().expect("clock read lock").clone()
    }
}

struct SequenceIds(RwLock<u32>);

impl IdGen for SequenceIds {
    fn new_id(&self) -> String {
        let mut value = self.0.write().expect("id write lock");
        *value += 1;
        format!("service-id-{value}")
    }
}

fn human(user: &User) -> TripAuthorizationContext {
    TripAuthorizationContext::human(user.id.clone())
}

fn place(id: &str) -> Place {
    Place {
        id: id.into(),
        name: format!("Place {id}"),
        kind: PlaceKind::Sight,
        lat: 0.0,
        lng: 0.0,
        tz: "UTC".into(),
        country_code: String::new(),
        admin_area: String::new(),
        city: "London".into(),
        address: "1 Test Street".into(),
        external_ref: None,
        website: None,
        phone: None,
        rating: None,
        price_level: None,
        opening_hours: None,
        photo_urls: Vec::new(),
        guide: None,
    }
}

fn candidate(id: &str, place_id: &str, proposer: &User) -> Candidate {
    Candidate {
        id: id.into(),
        trip_id: "trip-a".into(),
        source_place_id: None,
        place_id: place_id.into(),
        proposed_by: proposer.id.0.clone(),
        created_at: NOW.into(),
        pitch: "A good stop".into(),
        tags: vec!["quiet".into()],
        status: CandidateStatus::Shortlisted,
    }
}

fn plan() -> Plan {
    Plan {
        id: "plan-1".into(),
        trip_id: "trip-a".into(),
        version: 1,
        created_from_proposal_id: None,
        created_at: NOW.into(),
    }
}

fn days() -> Vec<Day> {
    ["2026-08-07", "2026-08-08", "2026-08-09"]
        .into_iter()
        .enumerate()
        .map(|(index, date)| Day {
            id: format!("day-{}", index + 1),
            plan_id: "plan-1".into(),
            date: date.into(),
            city_hint: "London".into(),
            tz: "UTC".into(),
            window_start: "09:00".into(),
            window_end: "21:00".into(),
        })
        .collect()
}

fn add_stop_proposal(id: &str, creator: &User, route: ProposalRoute) -> Proposal {
    Proposal {
        id: id.into(),
        trip_id: "trip-a".into(),
        created_by: creator.id.0.clone(),
        source: itinera_core::domain::content_history::ChangeSource::Web {},
        title: "Add the candidate".into(),
        rationale: "It fits the route".into(),
        change_set: ChangeSet {
            base_plan_version: 1,
            ops: vec![ChangeOp::AddStop {
                day_id: "day-1".into(),
                place_id: "candidate-place".into(),
                seq: 1.0,
                stop_kind: StopKind::Visit,
            }],
        },
        route,
        status: ProposalStatus::Pending,
        decided_by: None,
        rejection_reason: None,
        created_at: "2026-08-07T13:00:00.000Z".into(),
    }
}

fn decision_poll(id: &str, options: &[&str], allow_multi: bool) -> NewDecisionPoll {
    NewDecisionPoll {
        id: id.into(),
        title: format!("Decision {id}"),
        description: String::new(),
        options: options
            .iter()
            .map(|option| PollOption {
                id: (*option).into(),
                label: format!("Option {option}"),
                proposal_id: None,
            })
            .collect(),
        closes_at: "2026-08-08T12:00:00.000Z".into(),
        allow_multi,
        created_at: "2026-08-07T13:00:00.000Z".into(),
    }
}

fn plan_poll(id: &str, created_at: &str) -> NewPlanChangePoll {
    NewPlanChangePoll {
        poll_id: id.into(),
        adopt_option_id: format!("{id}-adopt"),
        keep_option_id: format!("{id}-keep"),
        created_at: created_at.into(),
        closes_at: "2026-08-14T13:00:00.000Z".into(),
    }
}

fn day_change_proposal(id: &str, creator: &User, base_version: u32, add: bool) -> Proposal {
    Proposal {
        id: id.into(),
        trip_id: "trip-a".into(),
        created_by: creator.id.0.clone(),
        source: itinera_core::domain::content_history::ChangeSource::Web {},
        title: "Change a day".into(),
        rationale: String::new(),
        change_set: ChangeSet {
            base_plan_version: base_version,
            ops: if add {
                vec![ChangeOp::AddDay {
                    date: "2026-08-10".into(),
                    city_hint: "London".into(),
                }]
            } else {
                vec![ChangeOp::RemoveDay {
                    day_id: format!("extra-day-{base_version}"),
                }]
            },
        },
        route: ProposalRoute::LeaderApproval,
        status: ProposalStatus::Pending,
        decided_by: None,
        rejection_reason: None,
        created_at: "2026-08-07T13:00:00.000Z".into(),
    }
}

fn application_ids(prefix: &str) -> ProposalApplicationIds {
    ProposalApplicationIds {
        plan_id: format!("{prefix}-plan"),
        entity_ids: (0..40)
            .map(|index| format!("{prefix}-entity-{index}"))
            .collect(),
        audit_ids: (0..100)
            .map(|index| format!("{prefix}-audit-{index}"))
            .collect(),
    }
}

async fn add_membership(database: &TestDatabase, user: &User, role: TripRole) {
    sqlx::query(
        "INSERT INTO trip_memberships (trip_id, user_id, role, joined_at, revision) \
         VALUES ('trip-a', ?, ?, ?, 1)",
    )
    .bind(&user.id.0)
    .bind(role.as_ref())
    .bind(NOW)
    .execute(database.db.pool())
    .await
    .unwrap();
}

async fn seed_graph(database: &TestDatabase) -> (User, User, User) {
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    let viewer = seed_user(&users, "viewer", "viewer@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    add_membership(database, &member, TripRole::Member).await;
    add_membership(database, &viewer, TripRole::Viewer).await;
    let candidate_place = place("candidate-place");
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate("candidate-a", &candidate_place.id, &leader),
            candidate_place.clone(),
        )
        .await
        .unwrap();
    trips
        .initialize_plan(
            "trip-a",
            &human(&leader),
            &candidate_place.id,
            plan(),
            days(),
        )
        .await
        .unwrap();
    (leader, member, viewer)
}

async fn insert_initial_stop(database: &TestDatabase, stop_id: &str, seq: f64) {
    let mut transaction = database.db.pool().begin().await.unwrap();
    sqlx::query("INSERT INTO stop_identities (trip_id, id) VALUES ('trip-a', ?)")
        .bind(stop_id)
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO plan_stops ( \
             trip_id, plan_version, id, day_id, seq, place_id, stop_kind, \
             planned_arrival, duration_min, notes, revision \
         ) VALUES ('trip-a', 1, ?, 'day-1', ?, 'candidate-place', 'visit', \
             '20:00', 60, '', 1)",
    )
    .bind(stop_id)
    .bind(seq)
    .execute(&mut *transaction)
    .await
    .unwrap();
    refresh_plan_structure_hash(&mut transaction, 1).await;
    transaction.commit().await.unwrap();
}

async fn refresh_plan_structure_hash(
    transaction: &mut Transaction<'_, Sqlite>,
    version: u32,
) -> String {
    let (plan_id, trip_id, stored_version, proposal_id, created_at): (
        String,
        String,
        i64,
        Option<String>,
        String,
    ) = sqlx::query_as(
        "SELECT id, trip_id, version, created_from_proposal_id, created_at \
         FROM plans WHERE trip_id = 'trip-a' AND version = ?",
    )
    .bind(i64::from(version))
    .fetch_one(&mut **transaction)
    .await
    .unwrap();
    let day_structure = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, plan_id, date FROM plan_days \
         WHERE trip_id = 'trip-a' AND plan_version = ? ORDER BY date, id",
    )
    .bind(i64::from(version))
    .fetch_all(&mut **transaction)
    .await
    .unwrap();
    let stop_rows = sqlx::query_as::<_, (String, String, f64, String, String)>(
        "SELECT id, day_id, seq, place_id, stop_kind FROM plan_stops \
         WHERE trip_id = 'trip-a' AND plan_version = ? ORDER BY day_id, seq, id",
    )
    .bind(i64::from(version))
    .fetch_all(&mut **transaction)
    .await
    .unwrap();
    let stop_structure = stop_rows
        .iter()
        .map(|(id, day_id, seq, place_id, stop_kind)| {
            (
                id.as_str(),
                day_id.as_str(),
                seq.to_bits(),
                place_id.as_str(),
                stop_kind.as_str(),
            )
        })
        .collect::<Vec<_>>();
    let encoded = serde_json::to_vec(&(
        plan_id.as_str(),
        trip_id.as_str(),
        u32::try_from(stored_version).unwrap(),
        proposal_id.as_deref(),
        created_at.as_str(),
        day_structure,
        stop_structure,
    ))
    .unwrap();
    let digest = Sha256::digest(encoded);
    let mut hash = String::with_capacity(64);
    for byte in digest {
        write!(&mut hash, "{byte:02x}").unwrap();
    }
    sqlx::query(
        "UPDATE plans SET structure_hash = ? \
         WHERE trip_id = 'trip-a' AND version = ?",
    )
    .bind(&hash)
    .bind(i64::from(version))
    .execute(&mut **transaction)
    .await
    .unwrap();
    hash
}

async fn insert_proposal_raw(transaction: &mut Transaction<'_, Sqlite>, proposal: &Proposal) {
    let (decision_kind, decision_user_id, decision_poll_id) = match &proposal.decided_by {
        None => (None, None, None),
        Some(ProposalDecision::Leader { user_id }) => {
            (Some("leader"), Some(user_id.as_str()), None)
        }
        Some(ProposalDecision::Poll { poll_id }) => (Some("poll"), None, Some(poll_id.as_str())),
    };
    sqlx::query(
        "INSERT INTO proposals ( \
             trip_id, id, created_by, source_kind, title, rationale, change_set_json, \
             route, status, decision_kind, decision_user_id, decision_poll_id, \
             rejection_reason, created_at, revision \
         ) VALUES (?, ?, ?, 'web', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
    )
    .bind(&proposal.trip_id)
    .bind(&proposal.id)
    .bind(&proposal.created_by)
    .bind(&proposal.title)
    .bind(&proposal.rationale)
    .bind(serde_json::to_string(&proposal.change_set).unwrap())
    .bind(match proposal.route {
        ProposalRoute::LeaderApproval => "leader_approval",
        ProposalRoute::Poll => "poll",
    })
    .bind(match proposal.status {
        ProposalStatus::Pending => "pending",
        ProposalStatus::Rejected => "rejected",
        ProposalStatus::Applied => "applied",
        ProposalStatus::Stale => "stale",
        ProposalStatus::Draft | ProposalStatus::Approved => panic!("unsupported stored status"),
    })
    .bind(decision_kind)
    .bind(decision_user_id)
    .bind(decision_poll_id)
    .bind(&proposal.rejection_reason)
    .bind(&proposal.created_at)
    .execute(&mut **transaction)
    .await
    .unwrap();
}

async fn insert_poll_raw(transaction: &mut Transaction<'_, Sqlite>, poll: &Poll, created_at: &str) {
    sqlx::query(
        "INSERT INTO polls ( \
             trip_id, id, created_by, kind, title, description, created_at, \
             opens_at, closes_at, decided_at, quorum, allow_multi, status, \
             resolution_note, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
    )
    .bind(&poll.trip_id)
    .bind(&poll.id)
    .bind(&poll.created_by)
    .bind(match poll.kind {
        PollKind::Decision => "decision",
        PollKind::PlanChange => "plan_change",
    })
    .bind(&poll.title)
    .bind(&poll.description)
    .bind(created_at)
    .bind(&poll.opens_at)
    .bind(&poll.closes_at)
    .bind(&poll.decided_at)
    .bind(i64::from(poll.quorum))
    .bind(i64::from(poll.allow_multi))
    .bind(match poll.status {
        PollStatus::Draft => "draft",
        PollStatus::Scheduled => "scheduled",
        PollStatus::Open => "open",
        PollStatus::Passed => "passed",
        PollStatus::Failed => "failed",
        PollStatus::Expired => "expired",
    })
    .bind(&poll.resolution_note)
    .execute(&mut **transaction)
    .await
    .unwrap();
    for (position, option) in poll.options.iter().enumerate() {
        sqlx::query(
            "INSERT INTO poll_options (trip_id, poll_id, id, position, label, proposal_id) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&poll.trip_id)
        .bind(&poll.id)
        .bind(&option.id)
        .bind(i64::try_from(position).unwrap())
        .bind(&option.label)
        .bind(&option.proposal_id)
        .execute(&mut **transaction)
        .await
        .unwrap();
    }
}

async fn append_plan_version_raw(
    transaction: &mut Transaction<'_, Sqlite>,
    leader: &User,
    version: u32,
) {
    let mut proposal = day_change_proposal(
        &format!("history-proposal-{version:04}"),
        leader,
        version - 1,
        version.is_multiple_of(2),
    );
    proposal.status = ProposalStatus::Applied;
    proposal.decided_by = Some(ProposalDecision::Leader {
        user_id: leader.id.0.clone(),
    });
    insert_proposal_raw(transaction, &proposal).await;
    let plan_id = format!("history-plan-{version:04}");
    let base_structure_hash: String = sqlx::query_scalar(
        "SELECT structure_hash FROM plans \
         WHERE trip_id = 'trip-a' AND version = ?",
    )
    .bind(i64::from(version - 1))
    .fetch_one(&mut **transaction)
    .await
    .unwrap();
    let application_entity_ids = if version.is_multiple_of(2) {
        vec![format!("extra-day-{version}")]
    } else {
        Vec::new()
    };
    sqlx::query(
        "INSERT INTO plans ( \
             trip_id, version, id, created_from_proposal_id, created_at, \
             applied_change_set_json, application_entity_ids_json, \
             structural_audits_json, base_structure_hash, structure_hash, revision \
         ) VALUES ( \
             'trip-a', ?, ?, ?, '2026-08-07T13:00:00.000Z', \
             ?, ?, '[]', ?, ?, 1 \
         )",
    )
    .bind(i64::from(version))
    .bind(&plan_id)
    .bind(&proposal.id)
    .bind(serde_json::to_string(&proposal.change_set).unwrap())
    .bind(serde_json::to_string(&application_entity_ids).unwrap())
    .bind(base_structure_hash)
    .bind("0".repeat(64))
    .execute(&mut **transaction)
    .await
    .unwrap();
    for (index, date) in ["2026-08-07", "2026-08-08", "2026-08-09"]
        .into_iter()
        .enumerate()
    {
        sqlx::query(
            "INSERT INTO plan_days ( \
                 trip_id, plan_version, id, plan_id, date, city_hint, tz, \
                 window_start, window_end, revision \
             ) VALUES ('trip-a', ?, ?, ?, ?, 'London', 'UTC', '09:00', '21:00', 1)",
        )
        .bind(i64::from(version))
        .bind(format!("day-{}", index + 1))
        .bind(&plan_id)
        .bind(date)
        .execute(&mut **transaction)
        .await
        .unwrap();
    }
    if version.is_multiple_of(2) {
        sqlx::query(
            "INSERT INTO plan_days ( \
                 trip_id, plan_version, id, plan_id, date, city_hint, tz, \
                 window_start, window_end, revision \
             ) VALUES ('trip-a', ?, ?, ?, '2026-08-10', 'London', 'UTC', \
                 '09:00', '21:00', 1)",
        )
        .bind(i64::from(version))
        .bind(format!("extra-day-{version}"))
        .bind(&plan_id)
        .execute(&mut **transaction)
        .await
        .unwrap();
    }
    refresh_plan_structure_hash(transaction, version).await;
}

async fn seed_plan_history(database: &TestDatabase, leader: &User, versions: u32) {
    let mut transaction = database.db.pool().begin().await.unwrap();
    for version in 2..=versions {
        append_plan_version_raw(&mut transaction, leader, version).await;
    }
    sqlx::query(
        "UPDATE trips SET current_plan_id = ?, current_plan_version = ?, revision = revision + 1 \
         WHERE id = 'trip-a'",
    )
    .bind(format!("history-plan-{versions:04}"))
    .bind(i64::from(versions))
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
}

fn boundary_proposals(creator: &User) -> Vec<Proposal> {
    (0..1_000)
        .map(|index| {
            let mut proposal = add_stop_proposal(
                &format!("proposal-{index:04}"),
                creator,
                ProposalRoute::LeaderApproval,
            );
            proposal.title = "Proposal".into();
            proposal.rationale.clear();
            proposal
        })
        .collect()
}

fn pad_proposals_to_limit(proposals: &mut [Proposal]) {
    let mut remaining = MAX_RESPONSE_BYTES - serde_json::to_vec(&*proposals).unwrap().len();
    for proposal in proposals {
        let addition = remaining.min(4_000 - proposal.rationale.len());
        proposal.rationale.push_str(&"x".repeat(addition));
        remaining -= addition;
        if remaining == 0 {
            return;
        }
    }
    panic!("proposal fixtures do not have enough safe padding capacity");
}

fn boundary_polls(creator: &User) -> Vec<Poll> {
    (0..1_000)
        .map(|index| {
            let id = format!("poll-{index:04}");
            Poll {
                id: id.clone(),
                trip_id: "trip-a".into(),
                created_by: creator.id.0.clone(),
                kind: PollKind::Decision,
                title: "Decision".into(),
                description: String::new(),
                options: vec![
                    PollOption {
                        id: format!("{id}-a"),
                        label: "A".into(),
                        proposal_id: None,
                    },
                    PollOption {
                        id: format!("{id}-b"),
                        label: "B".into(),
                        proposal_id: None,
                    },
                ],
                opens_at: None,
                closes_at: "2026-08-08T12:00:00.000Z".into(),
                decided_at: None,
                quorum: 1,
                allow_multi: false,
                status: PollStatus::Open,
                votes: Vec::new(),
                resolution_note: None,
            }
        })
        .collect()
}

fn pad_polls_to_limit(polls: &mut [Poll]) {
    let mut remaining = MAX_RESPONSE_BYTES - serde_json::to_vec(&*polls).unwrap().len();
    for poll in polls {
        let addition = remaining.min(4_000 - poll.description.len());
        poll.description.push_str(&"x".repeat(addition));
        remaining -= addition;
        if remaining == 0 {
            return;
        }
    }
    panic!("poll fixtures do not have enough safe padding capacity");
}

async fn add_action_boundary_candidates(database: &TestDatabase, leader: &User) {
    let trips = database.trips();
    for index in 1..18 {
        let candidate_place = place(&format!("boundary-place-{index}"));
        trips
            .add_candidate(
                "trip-a",
                &human(leader),
                candidate(
                    &format!("boundary-candidate-{index}"),
                    &candidate_place.id,
                    leader,
                ),
                candidate_place,
            )
            .await
            .unwrap();
    }
}

fn action_boundary_proposal(id: &str, creator: &User) -> Proposal {
    let mut proposal = add_stop_proposal(id, creator, ProposalRoute::Poll);
    proposal.change_set.ops = (0..18)
        .map(|index| ChangeOp::AddStop {
            day_id: "day-1".into(),
            place_id: if index == 0 {
                "candidate-place".into()
            } else {
                format!("boundary-place-{index}")
            },
            seq: f64::from(index + 1),
            stop_kind: StopKind::Visit,
        })
        .chain([
            ChangeOp::AddDay {
                date: "2026-08-10".into(),
                city_hint: "London".into(),
            },
            ChangeOp::AddDay {
                date: "2026-08-11".into(),
                city_hint: "London".into(),
            },
        ])
        .collect();
    proposal
}

#[tokio::test]
async fn leader_publication_is_atomic_and_records_reciprocal_candidate_provenance() {
    let database = TestDatabase::new().await;
    let (leader, _member, viewer) = seed_graph(&database).await;
    let proposals = database.proposals();
    let trips = database.trips();

    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&viewer),
                add_stop_proposal("proposal-viewer", &viewer, ProposalRoute::LeaderApproval),
                application_ids("viewer"),
            )
            .await,
        Err(ProposalRepoError::Forbidden)
    );
    let applied = proposals
        .create_proposal(
            "trip-a",
            &human(&leader),
            add_stop_proposal("proposal-a", &leader, ProposalRoute::LeaderApproval),
            application_ids("apply"),
        )
        .await
        .expect("leader proposal applies immediately");
    assert_eq!(applied.status, ProposalStatus::Applied);
    let current = trips
        .get_current_plan("trip-a", &human(&leader))
        .await
        .unwrap();
    assert_eq!(current.plan.version, 2);
    assert_eq!(
        current.plan.created_from_proposal_id.as_deref(),
        Some("proposal-a")
    );
    assert_eq!(current.stops.len(), 1);
    assert_eq!(
        trips
            .list_candidates("trip-a", &human(&leader))
            .await
            .unwrap()[0]
            .candidate
            .status,
        CandidateStatus::InPlan
    );
    let history = database
        .history()
        .list_history("trip-a", &human(&leader))
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].entity, EditEntity::Candidate);
    assert_eq!(history[0].field, "status");
    assert_eq!(history[0].status, EditStatus::Applied);
    let link: (String, String, String) = sqlx::query_as(
        "SELECT proposal_id, candidate_id, candidate_place_id \
         FROM proposal_content_edits WHERE trip_id = 'trip-a'",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        link,
        (
            "proposal-a".into(),
            "candidate-a".into(),
            "candidate-place".into()
        )
    );

    database.shutdown().await;
}

#[tokio::test]
async fn candidate_revert_cannot_break_an_applied_plan_binding() {
    let database = TestDatabase::new().await;
    let (leader, _member, _viewer) = seed_graph(&database).await;
    let trips = database.trips();
    let proposals = database.proposals();
    let history = database.history();
    let replacement = place("replacement-place");

    trips
        .update_candidate(
            "trip-a",
            &human(&leader),
            "candidate-a",
            CandidateUpdate {
                place: replacement.clone(),
                pitch: "A good stop".into(),
                tags: vec!["quiet".into()],
                changed_at: "2026-08-07T12:30:00.000Z".into(),
                change_id: "prepublication-place".into(),
            },
        )
        .await
        .unwrap();
    let mut proposal = add_stop_proposal("proposal-bind", &leader, ProposalRoute::LeaderApproval);
    let ChangeOp::AddStop { place_id, .. } = &mut proposal.change_set.ops[0] else {
        panic!("fixture must add a stop");
    };
    *place_id = replacement.id.clone();
    proposals
        .create_proposal("trip-a", &human(&leader), proposal, application_ids("bind"))
        .await
        .unwrap();

    assert_eq!(
        history
            .revert_edit(
                "trip-a",
                &human(&leader),
                "prepublication-place-00",
                "2026-08-07T14:00:00.000Z",
                "forbidden-place-revert",
            )
            .await,
        Err(ContentHistoryRepoError::Conflict)
    );
    let stored = trips
        .list_candidates("trip-a", &human(&leader))
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(stored.candidate.status, CandidateStatus::InPlan);
    assert_eq!(stored.place, replacement);
    let current = trips
        .get_current_plan("trip-a", &human(&leader))
        .await
        .unwrap();
    assert_eq!(current.stops[0].place_id, "replacement-place");
    let edits = history
        .list_history("trip-a", &human(&leader))
        .await
        .unwrap();
    assert!(edits.iter().all(|edit| edit.id != "forbidden-place-revert"));
    assert_eq!(
        edits
            .iter()
            .find(|edit| edit.id == "prepublication-place-00")
            .unwrap()
            .status,
        EditStatus::Applied
    );

    database.shutdown().await;
}

#[tokio::test]
async fn same_instant_candidate_place_writes_do_not_depend_on_lexical_edit_ids() {
    let database = TestDatabase::new().await;
    let (leader, _member, _viewer) = seed_graph(&database).await;
    let trips = database.trips();
    database
        .proposals()
        .create_proposal(
            "trip-a",
            &human(&leader),
            add_stop_proposal(
                "proposal-same-time-add",
                &leader,
                ProposalRoute::LeaderApproval,
            ),
            application_ids("same-time-add"),
        )
        .await
        .unwrap();
    let same_instant = "2026-08-07T15:00:00.000Z";
    let mut remove = day_change_proposal("proposal-same-time-remove", &leader, 2, false);
    remove.created_at = same_instant.into();
    remove.change_set.ops = vec![ChangeOp::RemoveStop {
        stop_id: "same-time-add-entity-0".into(),
    }];
    database
        .proposals()
        .create_proposal(
            "trip-a",
            &human(&leader),
            remove,
            application_ids("z-structural-remove"),
        )
        .await
        .unwrap();
    let current = trips
        .list_candidates("trip-a", &human(&leader))
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(current.candidate.status, CandidateStatus::Shortlisted);
    let mut revised_place = current.place.clone();
    revised_place.id = "same-time-revised-place".into();
    trips
        .update_candidate(
            "trip-a",
            &human(&leader),
            &current.candidate.id,
            CandidateUpdate {
                place: revised_place,
                pitch: current.candidate.pitch,
                tags: current.candidate.tags,
                changed_at: same_instant.into(),
                change_id: "a-place-after-structure".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        database
            .history()
            .list_history("trip-a", &human(&leader))
            .await
            .is_ok()
    );

    database.shutdown().await;
}

#[tokio::test]
async fn history_rejects_a_candidate_link_rebound_to_another_adopted_place() {
    let database = TestDatabase::new().await;
    let (leader, _member, _viewer) = seed_graph(&database).await;
    let trips = database.trips();
    let second_place = place("candidate-place-b");
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate("candidate-b", &second_place.id, &leader),
            second_place,
        )
        .await
        .unwrap();
    let mut proposal = add_stop_proposal(
        "proposal-two-candidates",
        &leader,
        ProposalRoute::LeaderApproval,
    );
    proposal.change_set.ops.push(ChangeOp::AddStop {
        day_id: "day-1".into(),
        place_id: "candidate-place-b".into(),
        seq: 2.0,
        stop_kind: StopKind::Visit,
    });
    database
        .proposals()
        .create_proposal(
            "trip-a",
            &human(&leader),
            proposal,
            application_ids("two-candidates"),
        )
        .await
        .unwrap();

    let edit_id: String = sqlx::query_scalar(
        "SELECT edit_id FROM proposal_content_edits \
         WHERE trip_id = 'trip-a' AND proposal_id = 'proposal-two-candidates' \
           AND candidate_id = 'candidate-a'",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    let mut transaction = database.db.pool().begin().await.unwrap();
    sqlx::query(
        "UPDATE proposal_content_edits SET candidate_place_id = 'candidate-place-b' \
         WHERE trip_id = 'trip-a' AND edit_id = ?",
    )
    .bind(&edit_id)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE content_edits SET proposal_candidate_place_id = 'candidate-place-b' \
         WHERE trip_id = 'trip-a' AND id = ?",
    )
    .bind(&edit_id)
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    assert_eq!(
        database
            .history()
            .list_history("trip-a", &human(&leader))
            .await,
        Err(ContentHistoryRepoError::CorruptData)
    );

    let forged_old = place("candidate-place-b");
    let mut forged_new = forged_old.clone();
    forged_new.id = "candidate-place".into();
    sqlx::query(
        "INSERT INTO content_edits ( \
             trip_id, id, entity, entity_id, field, old_value_json, new_value_json, \
             author_id, source_kind, status, created_at, revision \
         ) VALUES ( \
             'trip-a', 'forged-cross-owner-place', 'candidate', 'candidate-a', 'place', \
             ?, ?, ?, 'web', 'applied', '2026-08-07T16:00:00.000Z', 1 \
         )",
    )
    .bind(serde_json::to_string(&forged_old).unwrap())
    .bind(serde_json::to_string(&forged_new).unwrap())
    .bind(&leader.id.0)
    .execute(database.db.pool())
    .await
    .unwrap();

    let structural_audits: String = sqlx::query_scalar(
        "SELECT structural_audits_json FROM plans \
         WHERE trip_id = 'trip-a' \
           AND created_from_proposal_id = 'proposal-two-candidates'",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    let rebound_audits = structural_audits.replacen(
        "\"candidatePlaceId\":\"candidate-place\"",
        "\"candidatePlaceId\":\"candidate-place-b\"",
        1,
    );
    assert_ne!(rebound_audits, structural_audits);
    sqlx::query(
        "UPDATE plans SET structural_audits_json = ? \
         WHERE trip_id = 'trip-a' \
           AND created_from_proposal_id = 'proposal-two-candidates'",
    )
    .bind(&rebound_audits)
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        database
            .history()
            .list_history("trip-a", &human(&leader))
            .await,
        Err(ContentHistoryRepoError::CorruptData)
    );

    let mut transaction = database.db.pool().begin().await.unwrap();
    sqlx::query(
        "UPDATE proposal_content_edits SET candidate_place_id = 'candidate-place' \
         WHERE trip_id = 'trip-a' AND edit_id = ?",
    )
    .bind(&edit_id)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE content_edits SET proposal_candidate_place_id = 'candidate-place' \
         WHERE trip_id = 'trip-a' AND id = ?",
    )
    .bind(&edit_id)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE plans SET structural_audits_json = ? \
         WHERE trip_id = 'trip-a' \
           AND created_from_proposal_id = 'proposal-two-candidates'",
    )
    .bind(&structural_audits)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "DELETE FROM content_edits \
         WHERE trip_id = 'trip-a' AND id = 'forged-cross-owner-place'",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    database
        .proposals()
        .create_proposal(
            "trip-a",
            &human(&leader),
            day_change_proposal("proposal-preserve-candidates", &leader, 2, true),
            application_ids("preserve-candidates"),
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE proposal_content_edits \
         SET proposal_id = 'proposal-preserve-candidates' \
         WHERE trip_id = 'trip-a' AND edit_id = ?",
    )
    .bind(&edit_id)
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        database
            .history()
            .list_history("trip-a", &human(&leader))
            .await,
        Err(ContentHistoryRepoError::CorruptData)
    );

    database.shutdown().await;
}

#[tokio::test]
async fn generic_trip_reads_reject_stale_pointers_and_noncanonical_proposals() {
    let database = TestDatabase::new().await;
    let (leader, _member, _viewer) = seed_graph(&database).await;
    let trips = database.trips();
    database
        .proposals()
        .create_proposal(
            "trip-a",
            &human(&leader),
            add_stop_proposal("proposal-pointer", &leader, ProposalRoute::LeaderApproval),
            application_ids("pointer"),
        )
        .await
        .unwrap();

    sqlx::query(
        "UPDATE trips SET current_plan_id = 'plan-1', current_plan_version = 1, \
             revision = revision + 1 WHERE id = 'trip-a'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    );

    sqlx::query(
        "UPDATE trips SET current_plan_id = 'pointer-plan', current_plan_version = 2, \
             revision = revision + 1 WHERE id = 'trip-a'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE proposals SET change_set_json = \
             '{\"basePlanVersion\":\"1\",\"ops\":[]}' \
         WHERE trip_id = 'trip-a' AND id = 'proposal-pointer'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    );

    database.shutdown().await;
}

#[tokio::test]
async fn generic_trip_reads_validate_historical_poll_provenance() {
    let database = TestDatabase::new().await;
    let (leader, member, _viewer) = seed_graph(&database).await;
    let polls = database.polls();
    let proposal = add_stop_proposal("proposal-poll-history", &member, ProposalRoute::Poll);
    polls
        .create_proposal_poll(
            "trip-a",
            &human(&member),
            proposal,
            plan_poll("poll-history", "2026-08-07T13:00:00.000Z"),
            application_ids("poll-history-preflight"),
        )
        .await
        .unwrap();
    polls
        .cast_vote(
            "trip-a",
            &human(&leader),
            "poll-history",
            &["poll-history-adopt".into()],
            &FixedClock("2026-08-07T14:00:00.000Z"),
        )
        .await
        .unwrap();
    polls
        .close_poll(
            "trip-a",
            &human(&leader),
            "poll-history",
            "2026-08-07T15:00:00.000Z",
            application_ids("poll-history-apply"),
        )
        .await
        .unwrap();
    let mut latest = day_change_proposal("proposal-latest", &leader, 2, true);
    latest.created_at = "2026-08-07T16:00:00.000Z".into();
    database
        .proposals()
        .create_proposal("trip-a", &human(&leader), latest, application_ids("latest"))
        .await
        .unwrap();

    sqlx::query(
        "UPDATE poll_options SET proposal_id = NULL \
         WHERE trip_id = 'trip-a' AND poll_id = 'poll-history' \
           AND id = 'poll-history-adopt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        database.trips().get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    );

    database.shutdown().await;
}

#[tokio::test]
async fn generic_trip_reads_accept_bounded_transient_place_references() {
    let database = TestDatabase::new().await;
    let (leader, _member, _viewer) = seed_graph(&database).await;
    let replacement = place("transient-final-place");
    database
        .trips()
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate("transient-final-candidate", &replacement.id, &leader),
            replacement,
        )
        .await
        .unwrap();
    let mut proposal = add_stop_proposal(
        "proposal-transient-place",
        &leader,
        ProposalRoute::LeaderApproval,
    );
    proposal.change_set.ops.push(ChangeOp::SwapPlace {
        stop_id: "transient-place-entity-0".into(),
        new_place_id: "transient-final-place".into(),
    });
    database
        .proposals()
        .create_proposal(
            "trip-a",
            &human(&leader),
            proposal,
            application_ids("transient-place"),
        )
        .await
        .unwrap();
    assert_eq!(
        database
            .trips()
            .get_trip("trip-a", &human(&leader))
            .await
            .unwrap()
            .current_plan_id(),
        Some("transient-place-plan")
    );
    assert_eq!(
        database
            .trips()
            .list_trips(&human(&leader))
            .await
            .unwrap()
            .len(),
        1
    );

    let mut generated = day_change_proposal("proposal-transient-generated-place", &leader, 2, true);
    generated.change_set.ops = vec![
        ChangeOp::AddPlaceStop {
            day_id: "day-1".into(),
            seq: 1.0,
            stop_kind: StopKind::Meal,
            draft: NewPlaceDraft {
                name: "Transient cafe".into(),
                kind: PlaceKind::Food,
                city: "London".into(),
                note: String::new(),
                url: None,
                lat: None,
                lng: None,
            },
        },
        ChangeOp::RemoveDay {
            day_id: "day-1".into(),
        },
    ];
    let generated_ids = application_ids("transient-generated-place");
    let generated_place_id = generated_ids.entity_ids[0].clone();
    database
        .proposals()
        .create_proposal("trip-a", &human(&leader), generated, generated_ids)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM trip_places WHERE trip_id = 'trip-a' AND id = ?",
        )
        .bind(generated_place_id)
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        database
            .trips()
            .get_trip("trip-a", &human(&leader))
            .await
            .unwrap()
            .current_plan_id(),
        Some("transient-generated-place-plan")
    );
    assert_eq!(
        database
            .trips()
            .list_trips(&human(&leader))
            .await
            .unwrap()
            .len(),
        1
    );

    database.shutdown().await;
}

#[tokio::test]
async fn generated_day_ids_cannot_rebind_retired_history_entities() {
    let database = TestDatabase::new().await;
    let (leader, _member, _viewer) = seed_graph(&database).await;
    let proposals = database.proposals();
    let mut first_ids = application_ids("day-first");
    first_ids.entity_ids[0] = "retired-day".into();
    proposals
        .create_proposal(
            "trip-a",
            &human(&leader),
            day_change_proposal("proposal-add-day", &leader, 1, true),
            first_ids,
        )
        .await
        .unwrap();
    let mut remove = day_change_proposal("proposal-remove-day", &leader, 2, false);
    remove.change_set.ops = vec![ChangeOp::RemoveDay {
        day_id: "retired-day".into(),
    }];
    proposals
        .create_proposal(
            "trip-a",
            &human(&leader),
            remove,
            application_ids("day-remove"),
        )
        .await
        .unwrap();

    let mut reused_ids = application_ids("day-reuse");
    reused_ids.entity_ids[0] = "retired-day".into();
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&leader),
                day_change_proposal("proposal-reuse-day", &leader, 3, true),
                reused_ids,
            )
            .await,
        Err(ProposalRepoError::CorruptData)
    );
    assert_eq!(
        database
            .trips()
            .get_current_plan("trip-a", &human(&leader))
            .await
            .unwrap()
            .plan
            .version,
        3
    );
    assert!(
        proposals
            .list_proposals("trip-a", &human(&leader))
            .await
            .unwrap()
            .iter()
            .all(|proposal| proposal.id != "proposal-reuse-day")
    );

    database.shutdown().await;
}

#[tokio::test]
async fn decision_poll_freezes_editor_quorum_and_serializes_owned_ballots() {
    let database = TestDatabase::new().await;
    let (leader, member, viewer) = seed_graph(&database).await;
    let polls = database.polls();
    let poll = polls
        .create_decision_poll(
            "trip-a",
            &human(&member),
            NewDecisionPoll {
                id: "poll-decision".into(),
                title: "Dinner".into(),
                description: String::new(),
                options: vec![
                    PollOption {
                        id: "ramen".into(),
                        label: "Ramen".into(),
                        proposal_id: None,
                    },
                    PollOption {
                        id: "pizza".into(),
                        label: "Pizza".into(),
                        proposal_id: None,
                    },
                ],
                closes_at: "2026-08-08T12:00:00.000Z".into(),
                allow_multi: false,
                created_at: "2026-08-07T13:00:00.000Z".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(poll.quorum, 1);
    assert_eq!(
        polls
            .cast_vote(
                "trip-a",
                &human(&viewer),
                &poll.id,
                &["ramen".into()],
                &FixedClock("2026-08-07T14:00:00.000Z"),
            )
            .await,
        Err(PollRepoError::Forbidden)
    );
    let voted = polls
        .cast_vote(
            "trip-a",
            &human(&member),
            &poll.id,
            &["ramen".into()],
            &FixedClock("2026-08-07T14:00:00.000Z"),
        )
        .await
        .unwrap();
    assert_eq!(voted.votes.len(), 1);
    let repeated = polls
        .cast_vote(
            "trip-a",
            &human(&member),
            &poll.id,
            &["ramen".into()],
            &FixedClock("2026-08-07T15:00:00.000Z"),
        )
        .await
        .unwrap();
    assert_eq!(repeated.votes[0].at, "2026-08-07T14:00:00.000Z");
    let closed = polls
        .close_poll(
            "trip-a",
            &human(&leader),
            &poll.id,
            "2026-08-07T16:00:00.000Z",
            application_ids("unused"),
        )
        .await
        .unwrap();
    assert_eq!(closed.status, PollStatus::Passed);
    assert!(closed.resolution_note.is_none());

    database.shutdown().await;
}

#[tokio::test]
async fn create_poll_normalizes_contract_valid_zero_offset_deadlines() {
    let database = TestDatabase::new().await;
    let (_leader, member, _viewer) = seed_graph(&database).await;
    let repo = database.polls();
    let created = create_poll(
        &repo,
        &SequenceIds(RwLock::new(0)),
        &FixedClock("2026-08-07T13:00:00+00:00"),
        "trip-a",
        &human(&member),
        CreatePollInput {
            title: "Dinner".into(),
            description: String::new(),
            options: vec!["Ramen".into(), "Pizza".into()],
            closes_at: "2026-08-08T12:00:00+00:00".into(),
            allow_multi: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(created.closes_at, "2026-08-08T12:00:00Z");
    assert_eq!(
        repo.list_polls("trip-a", &human(&member)).await.unwrap()[0].closes_at,
        "2026-08-08T12:00:00Z"
    );

    database.shutdown().await;
}

#[tokio::test]
async fn passing_plan_poll_uses_the_same_stale_safe_publication_boundary() {
    let database = TestDatabase::new().await;
    let (leader, member, _viewer) = seed_graph(&database).await;
    let polls = database.polls();
    let proposals = database.proposals();
    let proposal = add_stop_proposal("proposal-poll", &member, ProposalRoute::Poll);
    let created = polls
        .create_proposal_poll(
            "trip-a",
            &human(&member),
            proposal,
            NewPlanChangePoll {
                poll_id: "poll-plan".into(),
                adopt_option_id: "adopt".into(),
                keep_option_id: "keep".into(),
                created_at: "2026-08-07T13:00:00.000Z".into(),
                closes_at: "2026-08-14T13:00:00.000Z".into(),
            },
            application_ids("preflight"),
        )
        .await
        .unwrap();
    assert_eq!(created.status, ProposalStatus::Pending);
    let poll = polls
        .list_polls("trip-a", &human(&leader))
        .await
        .unwrap()
        .pop()
        .unwrap();
    polls
        .cast_vote(
            "trip-a",
            &human(&leader),
            &poll.id,
            &["adopt".into()],
            &FixedClock("2026-08-07T14:00:00.000Z"),
        )
        .await
        .unwrap();
    let closed = polls
        .close_poll(
            "trip-a",
            &human(&leader),
            &poll.id,
            "2026-08-07T15:00:00.000Z",
            application_ids("poll-apply"),
        )
        .await
        .unwrap();
    assert_eq!(closed.status, PollStatus::Passed);
    assert_eq!(
        proposals
            .list_proposals("trip-a", &human(&leader))
            .await
            .unwrap()[0]
            .status,
        ProposalStatus::Applied
    );
    assert_eq!(
        database
            .trips()
            .get_current_plan("trip-a", &human(&leader))
            .await
            .unwrap()
            .plan
            .version,
        2
    );

    database.shutdown().await;
}

#[tokio::test]
async fn member_proposals_wait_for_a_leader_and_competing_revisions_become_stale() {
    let database = TestDatabase::new().await;
    let (leader, member, viewer) = seed_graph(&database).await;
    let proposals = database.proposals();

    for id in ["proposal-first", "proposal-competing"] {
        let pending = proposals
            .create_proposal(
                "trip-a",
                &human(&member),
                add_stop_proposal(id, &member, ProposalRoute::LeaderApproval),
                application_ids(id),
            )
            .await
            .unwrap();
        assert_eq!(pending.status, ProposalStatus::Pending);
        assert!(pending.decided_by.is_none());
    }
    assert_eq!(
        proposals
            .list_proposals("trip-a", &human(&viewer))
            .await
            .unwrap()
            .len(),
        2
    );

    let applied = proposals
        .approve_proposal(
            "trip-a",
            &human(&leader),
            "proposal-first",
            "2026-08-07T14:00:00.000Z",
            application_ids("first-apply"),
        )
        .await
        .unwrap();
    assert_eq!(applied.status, ProposalStatus::Applied);
    assert_eq!(
        proposals
            .approve_proposal(
                "trip-a",
                &human(&leader),
                "proposal-first",
                "2026-08-07T15:00:00.000Z",
                application_ids("ignored-retry"),
            )
            .await
            .unwrap(),
        applied
    );
    assert_eq!(
        proposals
            .approve_proposal(
                "trip-a",
                &human(&leader),
                "proposal-competing",
                "2026-08-07T15:00:00.000Z",
                application_ids("stale"),
            )
            .await,
        Err(ProposalRepoError::Conflict)
    );
    let competing = proposals
        .list_proposals("trip-a", &human(&leader))
        .await
        .unwrap()
        .into_iter()
        .find(|proposal| proposal.id == "proposal-competing")
        .unwrap();
    assert_eq!(competing.status, ProposalStatus::Stale);
    assert!(competing.decided_by.is_none());

    let mut rejectable =
        add_stop_proposal("proposal-reject", &member, ProposalRoute::LeaderApproval);
    rejectable.change_set.base_plan_version = 2;
    rejectable.created_at = "2026-08-07T16:00:00.000Z".into();
    proposals
        .create_proposal(
            "trip-a",
            &human(&member),
            rejectable,
            application_ids("reject-pending"),
        )
        .await
        .unwrap();
    let rejected = proposals
        .reject_proposal(
            "trip-a",
            &human(&leader),
            "proposal-reject",
            "Not this time",
        )
        .await
        .unwrap();
    assert_eq!(rejected.status, ProposalStatus::Rejected);
    assert_eq!(
        proposals
            .reject_proposal(
                "trip-a",
                &human(&leader),
                "proposal-reject",
                "A different retry reason is ignored",
            )
            .await
            .unwrap(),
        rejected
    );
    assert_eq!(
        proposals
            .reject_proposal(
                "trip-a",
                &human(&member),
                "proposal-competing",
                "not a leader",
            )
            .await,
        Err(ProposalRepoError::Forbidden)
    );

    database.shutdown().await;
}

#[tokio::test]
async fn pending_proposals_preflight_resource_binding_semantics_and_generated_ids() {
    let database = TestDatabase::new().await;
    let (_leader, member, _viewer) = seed_graph(&database).await;
    let proposals = database.proposals();

    let mut missing_day = add_stop_proposal(
        "proposal-missing-day",
        &member,
        ProposalRoute::LeaderApproval,
    );
    missing_day.change_set.ops = vec![ChangeOp::AddStop {
        day_id: "foreign-day".into(),
        place_id: "candidate-place".into(),
        seq: 1.0,
        stop_kind: StopKind::Visit,
    }];
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&member),
                missing_day,
                application_ids("missing-day"),
            )
            .await,
        Err(ProposalRepoError::NotFound)
    );

    let mut missing_place = add_stop_proposal(
        "proposal-missing-place",
        &member,
        ProposalRoute::LeaderApproval,
    );
    missing_place.change_set.ops = vec![ChangeOp::AddStop {
        day_id: "day-1".into(),
        place_id: "foreign-place".into(),
        seq: 1.0,
        stop_kind: StopKind::Visit,
    }];
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&member),
                missing_place,
                application_ids("missing-place"),
            )
            .await,
        Err(ProposalRepoError::NotFound)
    );

    let mut missing_stop = add_stop_proposal(
        "proposal-missing-stop",
        &member,
        ProposalRoute::LeaderApproval,
    );
    missing_stop.change_set.ops = vec![ChangeOp::MoveStop {
        stop_id: "foreign-stop".into(),
        to_day_id: "day-1".into(),
        seq: 1.0,
    }];
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&member),
                missing_stop,
                application_ids("missing-stop"),
            )
            .await,
        Err(ProposalRepoError::NotFound)
    );

    let mut collision_ids = application_ids("pending-collision");
    collision_ids.entity_ids[0] = "day-1".into();
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&member),
                day_change_proposal("proposal-day-collision", &member, 1, true),
                collision_ids,
            )
            .await,
        Err(ProposalRepoError::CorruptData)
    );

    insert_initial_stop(&database, "existing-noop-stop", 100.0).await;
    let mut no_op = add_stop_proposal(
        "proposal-semantic-noop",
        &member,
        ProposalRoute::LeaderApproval,
    );
    no_op.change_set.ops = vec![ChangeOp::MoveStop {
        stop_id: "existing-noop-stop".into(),
        to_day_id: "day-1".into(),
        seq: 100.0,
    }];
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&member),
                no_op,
                application_ids("semantic-noop"),
            )
            .await,
        Err(ProposalRepoError::InvalidChange)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM proposals WHERE trip_id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0
    );

    database.shutdown().await;
}

#[tokio::test]
async fn decision_poll_enforces_majority_ballot_shape_deadline_and_clock_order() {
    let database = TestDatabase::new().await;
    let (leader, member, _viewer) = seed_graph(&database).await;
    let users = database.users();
    let member_two = seed_user(&users, "member-two", "member-two@example.com").await;
    let member_three = seed_user(&users, "member-three", "member-three@example.com").await;
    add_membership(&database, &member_two, TripRole::Member).await;
    add_membership(&database, &member_three, TripRole::Member).await;
    let polls = database.polls();

    let draft = Poll {
        id: "poll-draft".into(),
        trip_id: "trip-a".into(),
        created_by: member.id.0.clone(),
        kind: PollKind::Decision,
        title: "Draft".into(),
        description: String::new(),
        options: vec![
            PollOption {
                id: "draft-a".into(),
                label: "A".into(),
                proposal_id: None,
            },
            PollOption {
                id: "draft-b".into(),
                label: "B".into(),
                proposal_id: None,
            },
        ],
        opens_at: None,
        closes_at: "2026-08-08T12:00:00.000Z".into(),
        decided_at: None,
        quorum: 2,
        allow_multi: false,
        status: PollStatus::Draft,
        votes: Vec::new(),
        resolution_note: None,
    };
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_poll_raw(&mut transaction, &draft, "2026-08-07T13:00:00.000Z").await;
    transaction.commit().await.unwrap();
    assert_eq!(
        polls
            .open_poll(
                "trip-a",
                &human(&member_two),
                &draft.id,
                &FixedClock("2026-08-07T14:00:00.000Z"),
            )
            .await,
        Err(PollRepoError::Forbidden)
    );
    assert_eq!(
        polls
            .open_poll(
                "trip-a",
                &human(&member),
                &draft.id,
                &FixedClock(&draft.closes_at),
            )
            .await,
        Err(PollRepoError::Conflict)
    );
    let opened = polls
        .open_poll(
            "trip-a",
            &human(&member),
            &draft.id,
            &FixedClock("2026-08-07T14:00:00.000Z"),
        )
        .await
        .unwrap();
    assert_eq!(opened.status, PollStatus::Open);
    assert_eq!(
        polls
            .open_poll(
                "trip-a",
                &human(&leader),
                &draft.id,
                &FixedClock("2026-08-08T12:00:00.000Z"),
            )
            .await
            .unwrap(),
        opened
    );

    let single = polls
        .create_decision_poll(
            "trip-a",
            &human(&member),
            decision_poll("poll-single", &["a", "b", "c"], false),
        )
        .await
        .unwrap();
    assert_eq!(single.quorum, 2);
    assert_eq!(
        polls
            .cast_vote(
                "trip-a",
                &human(&member),
                &single.id,
                &[],
                &FixedClock("2026-08-07T14:00:00.000Z"),
            )
            .await,
        Err(PollRepoError::InvalidVote)
    );
    assert_eq!(
        polls
            .cast_vote(
                "trip-a",
                &human(&member),
                &single.id,
                &["a".into()],
                &FixedClock("2026-08-07T12:59:59.000Z"),
            )
            .await,
        Err(PollRepoError::Conflict)
    );
    assert_eq!(
        polls
            .cast_vote(
                "trip-a",
                &human(&member),
                &single.id,
                &["a".into()],
                &FixedClock("2026-08-08T12:00:00.000Z"),
            )
            .await,
        Err(PollRepoError::Conflict)
    );
    for (actor, option, at) in [
        (&leader, "a", "2026-08-07T14:00:00.000Z"),
        (&member, "a", "2026-08-07T14:01:00.000Z"),
        (&member_two, "b", "2026-08-07T14:02:00.000Z"),
        (&member_three, "c", "2026-08-07T14:03:00.000Z"),
    ] {
        polls
            .cast_vote(
                "trip-a",
                &human(actor),
                &single.id,
                &[option.into()],
                &FixedClock(at),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        polls
            .close_poll(
                "trip-a",
                &human(&leader),
                &single.id,
                "2026-08-07T14:02:30.000Z",
                application_ids("clock-rollback"),
            )
            .await,
        Err(PollRepoError::Conflict)
    );
    let no_majority = polls
        .close_poll(
            "trip-a",
            &human(&leader),
            &single.id,
            "2026-08-07T15:00:00.000Z",
            application_ids("no-majority"),
        )
        .await
        .unwrap();
    assert_eq!(no_majority.status, PollStatus::Failed);
    assert_eq!(
        no_majority.resolution_note.as_deref(),
        Some("No option reached a majority - no decision recorded.")
    );

    let multi = polls
        .create_decision_poll(
            "trip-a",
            &human(&member),
            decision_poll("poll-multi", &["multi-a", "multi-b"], true),
        )
        .await
        .unwrap();
    let selected = polls
        .cast_vote(
            "trip-a",
            &human(&member),
            &multi.id,
            &["multi-b".into(), "multi-a".into()],
            &FixedClock("2026-08-07T14:00:00.000Z"),
        )
        .await
        .unwrap();
    assert_eq!(selected.votes.len(), 2);
    let withdrawn = polls
        .cast_vote(
            "trip-a",
            &human(&member),
            &multi.id,
            &[],
            &FixedClock("2026-08-07T15:00:00.000Z"),
        )
        .await
        .unwrap();
    assert!(withdrawn.votes.is_empty());

    database.shutdown().await;
}

#[tokio::test]
async fn poll_open_and_ballot_recheck_deadlines_after_waiting_for_the_writer_lock() {
    let database = TestDatabase::new().await;
    let (_leader, member, _viewer) = seed_graph(&database).await;
    let poll = database
        .polls()
        .create_decision_poll(
            "trip-a",
            &human(&member),
            decision_poll("poll-deadline-lock", &["deadline-a", "deadline-b"], false),
        )
        .await
        .unwrap();
    let mut draft = poll.clone();
    draft.id = "poll-open-deadline-lock".into();
    draft.title = "Open deadline".into();
    draft.status = PollStatus::Draft;
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_poll_raw(&mut transaction, &draft, "2026-08-07T13:00:00.000Z").await;
    transaction.commit().await.unwrap();

    let mut opening_blocker = raw_connection(&database.path).await;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut opening_blocker)
        .await
        .unwrap();
    let opening_clock = MutableClock::new("2026-08-08T11:59:59.999Z");
    let task_clock = opening_clock.clone();
    let barrier = Arc::new(Barrier::new(2));
    let task_barrier = barrier.clone();
    let opening_repo = database.polls();
    let opening_member = member.clone();
    let open = tokio::spawn(async move {
        task_barrier.wait().await;
        opening_repo
            .open_poll(
                "trip-a",
                &human(&opening_member),
                "poll-open-deadline-lock",
                &task_clock,
            )
            .await
    });
    barrier.wait().await;
    tokio::task::yield_now().await;
    opening_clock.set("2026-08-08T12:00:00.000Z");
    sqlx::query("ROLLBACK")
        .execute(&mut opening_blocker)
        .await
        .unwrap();
    drop(opening_blocker);
    assert_eq!(open.await.unwrap(), Err(PollRepoError::Conflict));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM polls \
             WHERE trip_id = 'trip-a' AND id = 'poll-open-deadline-lock'",
        )
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        "draft"
    );

    let mut blocker = raw_connection(&database.path).await;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut blocker)
        .await
        .unwrap();
    let clock = MutableClock::new("2026-08-08T11:59:59.999Z");
    let task_clock = clock.clone();
    let barrier = Arc::new(Barrier::new(2));
    let task_barrier = barrier.clone();
    let voter_repo = database.polls();
    let poll_id = poll.id.clone();
    let vote = tokio::spawn(async move {
        task_barrier.wait().await;
        voter_repo
            .cast_vote(
                "trip-a",
                &human(&member),
                &poll_id,
                &["deadline-a".into()],
                &task_clock,
            )
            .await
    });
    barrier.wait().await;
    tokio::task::yield_now().await;
    clock.set("2026-08-08T12:00:00.000Z");
    sqlx::query("ROLLBACK").execute(&mut blocker).await.unwrap();
    drop(blocker);
    assert_eq!(vote.await.unwrap(), Err(PollRepoError::Conflict));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM poll_ballots \
             WHERE trip_id = 'trip-a' AND poll_id = 'poll-deadline-lock'",
        )
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        0
    );

    database.shutdown().await;
}

#[tokio::test]
async fn no_decision_plan_poll_can_be_replaced_but_keep_and_stale_are_terminal() {
    let database = TestDatabase::new().await;
    let (leader, member, _viewer) = seed_graph(&database).await;
    let polls = database.polls();
    let proposals = database.proposals();

    proposals
        .create_proposal(
            "trip-a",
            &human(&member),
            add_stop_proposal("proposal-repoll", &member, ProposalRoute::LeaderApproval),
            application_ids("repoll-pending"),
        )
        .await
        .unwrap();
    let first = polls
        .route_proposal_to_poll(
            "trip-a",
            &human(&leader),
            "proposal-repoll",
            plan_poll("z-poll-first", "2026-08-07T13:00:00.000Z"),
            application_ids("first-preflight"),
        )
        .await
        .unwrap();
    let expired = polls
        .close_poll(
            "trip-a",
            &human(&leader),
            &first.id,
            "2026-08-07T14:00:00.000Z",
            application_ids("unused-expired"),
        )
        .await
        .unwrap();
    assert_eq!(expired.status, PollStatus::Expired);
    let replacement = polls
        .route_proposal_to_poll(
            "trip-a",
            &human(&leader),
            "proposal-repoll",
            plan_poll("a-poll-replacement", "2026-08-07T14:00:00.000Z"),
            application_ids("replacement-preflight"),
        )
        .await
        .unwrap();
    assert_eq!(replacement.status, PollStatus::Open);
    assert_eq!(
        polls
            .list_polls("trip-a", &human(&leader))
            .await
            .unwrap()
            .len(),
        2
    );
    sqlx::query(
        "UPDATE polls SET replaces_poll_id = NULL \
         WHERE trip_id = 'trip-a' AND id = 'a-poll-replacement'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        polls.list_polls("trip-a", &human(&leader)).await,
        Err(PollRepoError::CorruptData)
    );
    sqlx::query(
        "UPDATE polls SET replaces_poll_id = 'z-poll-first' \
         WHERE trip_id = 'trip-a' AND id = 'a-poll-replacement'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    polls
        .cast_vote(
            "trip-a",
            &human(&leader),
            &replacement.id,
            &["a-poll-replacement-keep".into()],
            &FixedClock("2026-08-07T16:00:00.000Z"),
        )
        .await
        .unwrap();
    let kept = polls
        .close_poll(
            "trip-a",
            &human(&leader),
            &replacement.id,
            "2026-08-07T17:00:00.000Z",
            application_ids("unused-keep"),
        )
        .await
        .unwrap();
    assert_eq!(kept.status, PollStatus::Failed);
    assert_eq!(
        proposals
            .list_proposals("trip-a", &human(&leader))
            .await
            .unwrap()[0]
            .status,
        ProposalStatus::Rejected
    );
    assert_eq!(
        polls
            .route_proposal_to_poll(
                "trip-a",
                &human(&leader),
                "proposal-repoll",
                plan_poll("poll-after-keep", "2026-08-07T18:00:00.000Z"),
                application_ids("after-keep"),
            )
            .await,
        Ok(kept)
    );

    let stale_proposal = add_stop_proposal("proposal-stale-poll", &member, ProposalRoute::Poll);
    polls
        .create_proposal_poll(
            "trip-a",
            &human(&member),
            stale_proposal,
            plan_poll("poll-stale", "2026-08-07T13:00:00.000Z"),
            application_ids("stale-preflight"),
        )
        .await
        .unwrap();
    proposals
        .create_proposal(
            "trip-a",
            &human(&leader),
            add_stop_proposal("proposal-advance", &leader, ProposalRoute::LeaderApproval),
            application_ids("advance"),
        )
        .await
        .unwrap();
    polls
        .cast_vote(
            "trip-a",
            &human(&leader),
            "poll-stale",
            &["poll-stale-adopt".into()],
            &FixedClock("2026-08-07T14:00:00.000Z"),
        )
        .await
        .unwrap();
    let stale = polls
        .close_poll(
            "trip-a",
            &human(&leader),
            "poll-stale",
            "2026-08-07T15:00:00.000Z",
            application_ids("stale-close"),
        )
        .await
        .unwrap();
    assert_eq!(stale.status, PollStatus::Failed);
    assert_eq!(
        proposals
            .list_proposals("trip-a", &human(&leader))
            .await
            .unwrap()
            .into_iter()
            .find(|proposal| proposal.id == "proposal-stale-poll")
            .unwrap()
            .status,
        ProposalStatus::Stale
    );

    database.shutdown().await;
}

#[tokio::test]
async fn governance_preserves_non_disclosure_roles_and_service_fail_closed_behavior() {
    let database = TestDatabase::new().await;
    let (leader, member, viewer) = seed_graph(&database).await;
    let users = database.users();
    let outsider = seed_user(&users, "outsider", "outsider@example.com").await;
    let proposals = database.proposals();
    let polls = database.polls();
    let service = TripAuthorizationContext::service(leader.id.clone(), "service-a".into());

    let pending = proposals
        .create_proposal(
            "trip-a",
            &human(&member),
            add_stop_proposal("proposal-pending", &member, ProposalRoute::LeaderApproval),
            application_ids("pending"),
        )
        .await
        .unwrap();
    let poll = polls
        .create_decision_poll(
            "trip-a",
            &human(&member),
            decision_poll("poll-auth", &["auth-a", "auth-b"], false),
        )
        .await
        .unwrap();

    assert_eq!(
        proposals.list_proposals("trip-a", &human(&outsider)).await,
        Err(ProposalRepoError::NotFound)
    );
    assert_eq!(
        polls.list_polls("trip-a", &human(&outsider)).await,
        Err(PollRepoError::NotFound)
    );
    assert_eq!(
        polls
            .create_decision_poll(
                "trip-a",
                &human(&viewer),
                decision_poll("poll-viewer", &["viewer-a", "viewer-b"], false),
            )
            .await,
        Err(PollRepoError::Forbidden)
    );
    assert_eq!(
        polls
            .close_poll(
                "trip-a",
                &human(&member),
                &poll.id,
                "2026-08-07T15:00:00.000Z",
                application_ids("member-close"),
            )
            .await,
        Err(PollRepoError::Forbidden)
    );

    assert_eq!(
        proposals.list_proposals("trip-a", &service).await,
        Err(ProposalRepoError::Forbidden)
    );
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &service,
                add_stop_proposal("proposal-service", &leader, ProposalRoute::LeaderApproval),
                application_ids("service-create"),
            )
            .await,
        Err(ProposalRepoError::Forbidden)
    );
    assert_eq!(
        proposals
            .approve_proposal(
                "trip-a",
                &service,
                &pending.id,
                "2026-08-07T15:00:00.000Z",
                application_ids("service-approve"),
            )
            .await,
        Err(ProposalRepoError::Forbidden)
    );
    assert_eq!(
        proposals
            .reject_proposal("trip-a", &service, &pending.id, "service rejection")
            .await,
        Err(ProposalRepoError::Forbidden)
    );
    assert_eq!(
        polls.list_polls("trip-a", &service).await,
        Err(PollRepoError::Forbidden)
    );
    assert_eq!(
        polls
            .create_decision_poll(
                "trip-a",
                &service,
                decision_poll("poll-service", &["service-a", "service-b"], false),
            )
            .await,
        Err(PollRepoError::Forbidden)
    );
    assert_eq!(
        polls
            .open_poll(
                "trip-a",
                &service,
                &poll.id,
                &FixedClock("2026-08-07T14:00:00.000Z"),
            )
            .await,
        Err(PollRepoError::Forbidden)
    );
    assert_eq!(
        polls
            .cast_vote(
                "trip-a",
                &service,
                &poll.id,
                &["auth-a".into()],
                &FixedClock("2026-08-07T14:00:00.000Z"),
            )
            .await,
        Err(PollRepoError::Forbidden)
    );
    assert_eq!(
        polls
            .close_poll(
                "trip-a",
                &service,
                &poll.id,
                "2026-08-07T15:00:00.000Z",
                application_ids("service-close"),
            )
            .await,
        Err(PollRepoError::Forbidden)
    );
    assert_eq!(
        polls
            .route_proposal_to_poll(
                "trip-a",
                &service,
                &pending.id,
                plan_poll("service-route", "2026-08-07T15:00:00.000Z"),
                application_ids("service-route"),
            )
            .await,
        Err(PollRepoError::Forbidden)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM proposals WHERE trip_id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM polls WHERE trip_id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        1
    );

    database.shutdown().await;
}

#[tokio::test]
async fn publication_and_poll_creation_roll_back_injected_failures_and_action_overflow() {
    let database = TestDatabase::new().await;
    let (leader, member, _viewer) = seed_graph(&database).await;
    let proposals = database.proposals();
    let polls = database.polls();
    let trips = database.trips();

    sqlx::query(
        "CREATE TRIGGER fail_governance_plan \
         BEFORE INSERT ON plans WHEN NEW.version > 1 \
         BEGIN SELECT RAISE(ABORT, 'injected plan failure'); END",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&leader),
                add_stop_proposal("proposal-rollback", &leader, ProposalRoute::LeaderApproval),
                application_ids("rollback"),
            )
            .await,
        Err(ProposalRepoError::Unavailable)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM proposals")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM plans WHERE trip_id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        trips
            .get_current_plan("trip-a", &human(&leader))
            .await
            .unwrap()
            .plan
            .version,
        1
    );
    assert_eq!(
        trips
            .list_candidates("trip-a", &human(&leader))
            .await
            .unwrap()[0]
            .candidate
            .status,
        CandidateStatus::Shortlisted
    );
    sqlx::query("DROP TRIGGER fail_governance_plan")
        .execute(database.db.pool())
        .await
        .unwrap();

    sqlx::query(
        "CREATE TRIGGER fail_poll_option \
         BEFORE INSERT ON poll_options WHEN NEW.poll_id = 'poll-rollback' \
         BEGIN SELECT RAISE(ABORT, 'injected option failure'); END",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        polls
            .create_decision_poll(
                "trip-a",
                &human(&leader),
                decision_poll("poll-rollback", &["rollback-a", "rollback-b"], false),
            )
            .await,
        Err(PollRepoError::Unavailable)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM polls")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM poll_options")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER fail_poll_option")
        .execute(database.db.pool())
        .await
        .unwrap();

    polls
        .create_proposal_poll(
            "trip-a",
            &human(&member),
            add_stop_proposal("proposal-close-rollback", &member, ProposalRoute::Poll),
            plan_poll("poll-close-rollback", "2026-08-07T13:00:00.000Z"),
            application_ids("close-preflight"),
        )
        .await
        .unwrap();
    polls
        .cast_vote(
            "trip-a",
            &human(&leader),
            "poll-close-rollback",
            &["poll-close-rollback-adopt".into()],
            &FixedClock("2026-08-07T14:00:00.000Z"),
        )
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_proposal_audit_link \
         BEFORE INSERT ON proposal_content_edits \
         BEGIN SELECT RAISE(ABORT, 'injected reciprocal audit failure'); END",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        polls
            .close_poll(
                "trip-a",
                &human(&leader),
                "poll-close-rollback",
                "2026-08-07T15:00:00.000Z",
                application_ids("close-rollback"),
            )
            .await,
        Err(PollRepoError::Unavailable)
    );
    assert_eq!(
        polls.list_polls("trip-a", &human(&leader)).await.unwrap()[0].status,
        PollStatus::Open
    );
    assert_eq!(
        proposals
            .list_proposals("trip-a", &human(&leader))
            .await
            .unwrap()[0]
            .status,
        ProposalStatus::Pending
    );
    assert_eq!(
        trips
            .get_current_plan("trip-a", &human(&leader))
            .await
            .unwrap()
            .plan
            .version,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM content_edits")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER fail_proposal_audit_link")
        .execute(database.db.pool())
        .await
        .unwrap();
    let mut transaction = database.db.pool().begin().await.unwrap();
    for statement in [
        "DELETE FROM poll_ballot_options",
        "DELETE FROM poll_ballots",
        "DELETE FROM poll_options",
        "DELETE FROM polls",
        "DELETE FROM proposals",
    ] {
        sqlx::query(statement)
            .execute(&mut *transaction)
            .await
            .unwrap();
    }
    transaction.commit().await.unwrap();

    for index in 1..20 {
        let candidate_place = place(&format!("candidate-place-{index}"));
        trips
            .add_candidate(
                "trip-a",
                &human(&leader),
                candidate(&format!("candidate-{index}"), &candidate_place.id, &leader),
                candidate_place,
            )
            .await
            .unwrap();
    }
    let mut oversized = add_stop_proposal(
        "proposal-action-overflow",
        &leader,
        ProposalRoute::LeaderApproval,
    );
    oversized.change_set.ops = (0..20)
        .map(|index| ChangeOp::AddStop {
            day_id: "day-1".into(),
            place_id: if index == 0 {
                "candidate-place".into()
            } else {
                format!("candidate-place-{index}")
            },
            seq: f64::from(index + 1),
            stop_kind: StopKind::Visit,
        })
        .collect();
    let mut pending_oversized = oversized.clone();
    pending_oversized.id = "proposal-pending-action-overflow".into();
    pending_oversized.created_by = member.id.0.clone();
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&member),
                pending_oversized,
                application_ids("pending-action-overflow"),
            )
            .await,
        Err(ProposalRepoError::SafetyLimitExceeded)
    );
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&leader),
                oversized,
                application_ids("action-overflow"),
            )
            .await,
        Err(ProposalRepoError::SafetyLimitExceeded)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM proposals")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM candidates WHERE trip_id = 'trip-a' AND status = 'shortlisted'",
        )
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        20
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM content_edits")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0
    );

    database.shutdown().await;
}

#[tokio::test]
async fn poll_publication_counts_its_terminal_update_at_the_100_action_boundary() {
    let allowed = TestDatabase::new().await;
    let (leader, member, _viewer) = seed_graph(&allowed).await;
    insert_initial_stop(&allowed, "existing-stop-a", 100.0).await;
    add_action_boundary_candidates(&allowed, &leader).await;
    let polls = allowed.polls();
    polls
        .create_proposal_poll(
            "trip-a",
            &human(&member),
            action_boundary_proposal("proposal-100-actions", &member),
            plan_poll("poll-100-actions", "2026-08-07T13:00:00.000Z"),
            application_ids("actions-100-preflight"),
        )
        .await
        .unwrap();
    polls
        .cast_vote(
            "trip-a",
            &human(&leader),
            "poll-100-actions",
            &["poll-100-actions-adopt".into()],
            &FixedClock("2026-08-07T14:00:00.000Z"),
        )
        .await
        .unwrap();
    assert_eq!(
        polls
            .close_poll(
                "trip-a",
                &human(&leader),
                "poll-100-actions",
                "2026-08-07T15:00:00.000Z",
                application_ids("actions-100-apply"),
            )
            .await
            .unwrap()
            .status,
        PollStatus::Passed
    );
    assert_eq!(
        allowed
            .trips()
            .get_current_plan("trip-a", &human(&leader))
            .await
            .unwrap()
            .plan
            .version,
        2
    );
    allowed.shutdown().await;

    let rejected = TestDatabase::new().await;
    let (leader, member, _viewer) = seed_graph(&rejected).await;
    insert_initial_stop(&rejected, "existing-stop-a", 100.0).await;
    insert_initial_stop(&rejected, "existing-stop-b", 101.0).await;
    add_action_boundary_candidates(&rejected, &leader).await;
    assert_eq!(
        rejected
            .polls()
            .create_proposal_poll(
                "trip-a",
                &human(&member),
                action_boundary_proposal("proposal-101-actions", &member),
                plan_poll("poll-101-actions", "2026-08-07T13:00:00.000Z"),
                application_ids("actions-101-preflight"),
            )
            .await,
        Err(PollRepoError::SafetyLimitExceeded)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM proposals")
            .fetch_one(rejected.db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM polls")
            .fetch_one(rejected.db.pool())
            .await
            .unwrap(),
        0
    );
    rejected.shutdown().await;

    let upgraded = TestDatabase::new().await;
    let (leader, member, _viewer) = seed_graph(&upgraded).await;
    insert_initial_stop(&upgraded, "existing-stop-a", 100.0).await;
    add_action_boundary_candidates(&upgraded, &leader).await;
    sqlx::query(
        "UPDATE plans SET structure_hash = NULL \
         WHERE trip_id = 'trip-a' AND version = 1",
    )
    .execute(upgraded.db.pool())
    .await
    .unwrap();
    assert_eq!(
        upgraded
            .polls()
            .create_proposal_poll(
                "trip-a",
                &human(&member),
                action_boundary_proposal("proposal-upgrade-101-actions", &member),
                plan_poll("poll-upgrade-101-actions", "2026-08-07T13:00:00.000Z"),
                application_ids("upgrade-101-preflight"),
            )
            .await,
        Err(PollRepoError::SafetyLimitExceeded)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT structure_hash IS NULL FROM plans \
             WHERE trip_id = 'trip-a' AND version = 1",
        )
        .fetch_one(upgraded.db.pool())
        .await
        .unwrap(),
        1
    );
    upgraded.shutdown().await;
}

#[tokio::test]
async fn governance_readers_reject_schema_valid_but_inconsistent_graphs() {
    let database = TestDatabase::new().await;
    let (leader, member, _viewer) = seed_graph(&database).await;
    let proposals = database.proposals();
    let polls = database.polls();
    let trips = database.trips();

    let decision = polls
        .create_decision_poll(
            "trip-a",
            &human(&member),
            decision_poll("poll-corrupt", &["corrupt-a", "corrupt-b"], false),
        )
        .await
        .unwrap();
    polls
        .cast_vote(
            "trip-a",
            &human(&member),
            &decision.id,
            &["corrupt-a".into()],
            &FixedClock("2026-08-07T14:00:00.000Z"),
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE poll_options SET position = 2 \
         WHERE trip_id = 'trip-a' AND poll_id = 'poll-corrupt' AND id = 'corrupt-b'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        polls.list_polls("trip-a", &human(&leader)).await,
        Err(PollRepoError::CorruptData)
    );
    sqlx::query(
        "UPDATE poll_options SET position = 1 \
         WHERE trip_id = 'trip-a' AND poll_id = 'poll-corrupt' AND id = 'corrupt-b'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE poll_ballots SET voted_at = '2026-08-07T12:59:59.000Z' \
         WHERE trip_id = 'trip-a' AND poll_id = 'poll-corrupt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        polls.list_polls("trip-a", &human(&leader)).await,
        Err(PollRepoError::CorruptData)
    );
    sqlx::query(
        "UPDATE poll_ballots SET voted_at = '2026-08-07T14:00:00.000Z' \
         WHERE trip_id = 'trip-a' AND poll_id = 'poll-corrupt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();

    sqlx::query(
        "UPDATE polls SET revision = 9223372036854775807 \
         WHERE trip_id = 'trip-a' AND id = 'poll-corrupt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        polls
            .cast_vote(
                "trip-a",
                &human(&member),
                &decision.id,
                &["corrupt-b".into()],
                &FixedClock("2026-08-07T15:00:00.000Z"),
            )
            .await,
        Err(PollRepoError::CorruptData)
    );
    sqlx::query(
        "UPDATE polls SET revision = 2 \
         WHERE trip_id = 'trip-a' AND id = 'poll-corrupt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE poll_ballots SET revision = 9223372036854775807 \
         WHERE trip_id = 'trip-a' AND poll_id = 'poll-corrupt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        polls
            .cast_vote(
                "trip-a",
                &human(&member),
                &decision.id,
                &["corrupt-b".into()],
                &FixedClock("2026-08-07T15:00:00.000Z"),
            )
            .await,
        Err(PollRepoError::CorruptData)
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT option_id FROM poll_ballot_options \
             WHERE trip_id = 'trip-a' AND poll_id = 'poll-corrupt' AND user_id = 'member'",
        )
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        "corrupt-a"
    );
    sqlx::query(
        "UPDATE poll_ballots SET revision = 1 \
         WHERE trip_id = 'trip-a' AND poll_id = 'poll-corrupt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();

    proposals
        .create_proposal(
            "trip-a",
            &human(&member),
            add_stop_proposal("proposal-revision", &member, ProposalRoute::LeaderApproval),
            application_ids("revision-pending"),
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE proposals SET revision = 9223372036854775807 \
         WHERE trip_id = 'trip-a' AND id = 'proposal-revision'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        proposals
            .reject_proposal("trip-a", &human(&leader), "proposal-revision", "No",)
            .await,
        Err(ProposalRepoError::CorruptData)
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM proposals \
             WHERE trip_id = 'trip-a' AND id = 'proposal-revision'",
        )
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        "pending"
    );

    proposals
        .create_proposal(
            "trip-a",
            &human(&leader),
            add_stop_proposal("proposal-corrupt", &leader, ProposalRoute::LeaderApproval),
            application_ids("corrupt-apply"),
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE proposals SET \
             source_kind = 'service', source_service_id = 'service-a', \
             source_service_name = 'Service A' \
         WHERE trip_id = 'trip-a' AND id = 'proposal-corrupt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        proposals.list_proposals("trip-a", &human(&leader)).await,
        Err(ProposalRepoError::CorruptData)
    );
    sqlx::query(
        "UPDATE proposals SET \
             source_kind = 'web', source_service_id = NULL, source_service_name = NULL \
         WHERE trip_id = 'trip-a' AND id = 'proposal-corrupt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE proposals SET status = 'stale' \
         WHERE trip_id = 'trip-a' AND id = 'proposal-corrupt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        proposals.list_proposals("trip-a", &human(&leader)).await,
        Err(ProposalRepoError::CorruptData)
    );
    sqlx::query(
        "UPDATE proposals SET status = 'applied' \
         WHERE trip_id = 'trip-a' AND id = 'proposal-corrupt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE poll_options SET proposal_id = 'proposal-corrupt' \
         WHERE trip_id = 'trip-a' AND poll_id = 'poll-corrupt' AND id = 'corrupt-a'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        proposals.list_proposals("trip-a", &human(&leader)).await,
        Err(ProposalRepoError::CorruptData)
    );
    assert_eq!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    );
    sqlx::query(
        "UPDATE poll_options SET proposal_id = NULL \
         WHERE trip_id = 'trip-a' AND poll_id = 'poll-corrupt' AND id = 'corrupt-a'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE candidates SET status = 'shortlisted' \
         WHERE trip_id = 'trip-a' AND id = 'candidate-a'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(
        trips
            .list_candidates("trip-a", &human(&leader))
            .await
            .is_err()
    );
    sqlx::query(
        "UPDATE candidates SET status = 'in_plan' \
         WHERE trip_id = 'trip-a' AND id = 'candidate-a'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    let structural_edit_id: String = sqlx::query_scalar(
        "SELECT edit_id FROM proposal_content_edits \
         WHERE trip_id = 'trip-a' AND proposal_id = 'proposal-corrupt'",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "DELETE FROM proposal_content_edits \
         WHERE trip_id = 'trip-a' AND proposal_id = 'proposal-corrupt'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(
        database
            .history()
            .list_history("trip-a", &human(&leader))
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM content_edits WHERE trip_id = 'trip-a' AND id = ?")
        .bind(structural_edit_id)
        .execute(database.db.pool())
        .await
        .unwrap();
    assert_eq!(
        database
            .history()
            .list_history("trip-a", &human(&leader))
            .await,
        Err(ContentHistoryRepoError::CorruptData)
    );

    database.shutdown().await;
}

#[tokio::test]
async fn schema_rejects_cross_trip_governance_links_and_wrong_storage_classes() {
    let database = TestDatabase::new().await;
    let (leader, member, _viewer) = seed_graph(&database).await;
    let users = database.users();
    let trips = database.trips();
    let other = seed_user(&users, "other", "other@example.com").await;
    seed_trip(&trips, "trip-b", &other).await;
    let proposals = database.proposals();
    let polls = database.polls();
    proposals
        .create_proposal(
            "trip-a",
            &human(&member),
            add_stop_proposal("proposal-cross", &member, ProposalRoute::LeaderApproval),
            application_ids("cross"),
        )
        .await
        .unwrap();
    polls
        .create_decision_poll(
            "trip-a",
            &human(&leader),
            decision_poll("poll-a", &["poll-a-1", "poll-a-2"], false),
        )
        .await
        .unwrap();
    polls
        .create_decision_poll(
            "trip-b",
            &human(&other),
            NewDecisionPoll {
                id: "poll-b".into(),
                title: "Other trip".into(),
                description: String::new(),
                options: vec![
                    PollOption {
                        id: "poll-b-1".into(),
                        label: "One".into(),
                        proposal_id: None,
                    },
                    PollOption {
                        id: "poll-b-2".into(),
                        label: "Two".into(),
                        proposal_id: None,
                    },
                ],
                closes_at: "2026-08-08T12:00:00.000Z".into(),
                allow_multi: false,
                created_at: "2026-08-07T13:00:00.000Z".into(),
            },
        )
        .await
        .unwrap();

    assert!(
        sqlx::query(
            "UPDATE poll_options SET proposal_id = 'proposal-cross' \
         WHERE trip_id = 'trip-b' AND poll_id = 'poll-b' AND id = 'poll-b-1'",
        )
        .execute(database.db.pool())
        .await
        .is_err()
    );
    let mut transaction = database.db.pool().begin().await.unwrap();
    sqlx::query(
        "INSERT INTO poll_ballots (trip_id, poll_id, user_id, voted_at, revision) \
         VALUES ('trip-a', 'poll-a', 'leader', '2026-08-07T14:00:00.000Z', 1)",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    assert!(
        sqlx::query(
            "INSERT INTO poll_ballot_options (trip_id, poll_id, user_id, option_id) \
         VALUES ('trip-a', 'poll-a', 'leader', 'poll-b-1')",
        )
        .execute(&mut *transaction)
        .await
        .is_err()
    );
    transaction.rollback().await.unwrap();
    for statement in [
        "INSERT INTO polls (trip_id, id, created_by, kind, title, description, created_at, closes_at, quorum, allow_multi, status, revision) VALUES ('trip-a', X'CAFE', 'leader', 'decision', 'Bad', '', '2026-08-07T13:00:00Z', '2026-08-08T13:00:00Z', 1, 0, 'open', 1)",
        "INSERT INTO polls (trip_id, id, created_by, kind, title, description, created_at, closes_at, quorum, allow_multi, status, revision) VALUES ('trip-a', 'bad-kind', 'leader', 'survey', 'Bad', '', '2026-08-07T13:00:00Z', '2026-08-08T13:00:00Z', 1, 0, 'open', 1)",
        "INSERT INTO proposals (trip_id, id, created_by, source_kind, title, rationale, change_set_json, route, status, created_at, revision) VALUES ('trip-a', X'CAFE', 'leader', 'web', 'Bad', '', '{}', 'leader_approval', 'pending', '2026-08-07T13:00:00Z', 1)",
    ] {
        assert!(
            sqlx::query(statement)
                .execute(database.db.pool())
                .await
                .is_err(),
            "schema unexpectedly accepted {statement}"
        );
    }

    database.shutdown().await;
}

#[tokio::test]
async fn governance_writers_serialize_role_revocation_and_ballot_close_races() {
    let database = Arc::new(TestDatabase::new().await);
    let (leader, member, _viewer) = seed_graph(&database).await;
    let proposals = database.proposals();
    let polls = database.polls();

    let mut blocker = raw_connection(&database.path).await;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut blocker)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE trip_memberships SET role = 'viewer', revision = revision + 1 \
         WHERE trip_id = 'trip-a' AND user_id = 'member'",
    )
    .execute(&mut blocker)
    .await
    .unwrap();
    let proposal_repo = proposals.clone();
    let revoked_actor = member.clone();
    let revoked = tokio::spawn(async move {
        proposal_repo
            .create_proposal(
                "trip-a",
                &human(&revoked_actor),
                add_stop_proposal(
                    "proposal-revoked",
                    &revoked_actor,
                    ProposalRoute::LeaderApproval,
                ),
                application_ids("revoked"),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    sqlx::query("COMMIT").execute(&mut blocker).await.unwrap();
    blocker.close().await.unwrap();
    assert_eq!(revoked.await.unwrap(), Err(ProposalRepoError::Forbidden));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM proposals WHERE trip_id = 'trip-a' AND id = 'proposal-revoked'",
        )
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        0
    );

    sqlx::query(
        "UPDATE trip_memberships SET role = 'member', revision = revision + 1 \
         WHERE trip_id = 'trip-a' AND user_id = 'member'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    let poll = polls
        .create_decision_poll(
            "trip-a",
            &human(&member),
            decision_poll("poll-race", &["race-a", "race-b"], false),
        )
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let voter_repo = polls.clone();
    let voter = member.clone();
    let voter_barrier = barrier.clone();
    let poll_id = poll.id.clone();
    let vote = tokio::spawn(async move {
        voter_barrier.wait().await;
        voter_repo
            .cast_vote(
                "trip-a",
                &human(&voter),
                &poll_id,
                &["race-a".into()],
                &FixedClock("2026-08-07T14:00:00.000Z"),
            )
            .await
    });
    let closer_repo = polls.clone();
    let closer = leader.clone();
    let closer_barrier = barrier.clone();
    let poll_id = poll.id.clone();
    let close = tokio::spawn(async move {
        closer_barrier.wait().await;
        closer_repo
            .close_poll(
                "trip-a",
                &human(&closer),
                &poll_id,
                "2026-08-07T15:00:00.000Z",
                application_ids("race-close"),
            )
            .await
    });
    barrier.wait().await;
    let vote = vote.await.unwrap();
    let closed = close.await.unwrap().unwrap();
    match vote {
        Ok(voted) => {
            assert_eq!(voted.votes.len(), 1);
            assert_eq!(closed.status, PollStatus::Passed);
        }
        Err(PollRepoError::Conflict) => {
            assert_eq!(closed.status, PollStatus::Expired);
        }
        other => panic!("unexpected vote result: {other:?}"),
    }
    let stored = polls
        .list_polls("trip-a", &human(&leader))
        .await
        .unwrap()
        .into_iter()
        .find(|stored| stored.id == poll.id)
        .unwrap();
    assert_eq!(stored, closed);

    drop(polls);
    drop(proposals);
    let database = Arc::try_unwrap(database)
        .ok()
        .expect("all test handles dropped");
    database.shutdown().await;
}

#[tokio::test]
async fn proposal_collection_enforces_exact_row_and_encoded_byte_boundaries() {
    let database = TestDatabase::new().await;
    let (leader, member, _viewer) = seed_graph(&database).await;
    let proposals = database.proposals();

    let mut expected = boundary_proposals(&leader);
    pad_proposals_to_limit(&mut expected);
    assert_eq!(
        serde_json::to_vec(&expected).unwrap().len(),
        MAX_RESPONSE_BYTES
    );
    let mut transaction = database.db.pool().begin().await.unwrap();
    for proposal in &expected {
        insert_proposal_raw(&mut transaction, proposal).await;
    }
    transaction.commit().await.unwrap();
    expected.sort_by(|left, right| right.id.cmp(&left.id));
    let exact = proposals
        .list_proposals("trip-a", &human(&leader))
        .await
        .expect("exactly 1,000 proposals and 4 MiB are accepted");
    assert_eq!(exact, expected);
    assert_eq!(
        serde_json::to_vec(&exact).unwrap().len(),
        MAX_RESPONSE_BYTES
    );

    let extendable = expected
        .iter()
        .find(|proposal| proposal.rationale.len() < 4_000)
        .unwrap();
    sqlx::query(
        "UPDATE proposals SET rationale = rationale || 'x' \
         WHERE trip_id = 'trip-a' AND id = ?",
    )
    .bind(&extendable.id)
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        proposals.list_proposals("trip-a", &human(&leader)).await,
        Err(ProposalRepoError::SafetyLimitExceeded)
    );
    sqlx::query(
        "UPDATE proposals SET rationale = substr(rationale, 1, length(rationale) - 1) \
         WHERE trip_id = 'trip-a' AND id = ?",
    )
    .bind(&extendable.id)
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&member),
                add_stop_proposal("proposal-overflow", &member, ProposalRoute::LeaderApproval,),
                application_ids("proposal-overflow"),
            )
            .await,
        Err(ProposalRepoError::SafetyLimitExceeded)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM proposals WHERE trip_id = 'trip-a'",)
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        1_000
    );
    let mut extra = add_stop_proposal("proposal-1000", &leader, ProposalRoute::LeaderApproval);
    extra.title = "Proposal".into();
    extra.rationale.clear();
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_proposal_raw(&mut transaction, &extra).await;
    transaction.commit().await.unwrap();
    assert_eq!(
        proposals.list_proposals("trip-a", &human(&leader)).await,
        Err(ProposalRepoError::SafetyLimitExceeded)
    );

    database.shutdown().await;
}

#[tokio::test]
async fn poll_collection_enforces_exact_row_and_encoded_byte_boundaries() {
    let database = TestDatabase::new().await;
    let (leader, _member, _viewer) = seed_graph(&database).await;
    let polls = database.polls();

    let mut expected = boundary_polls(&leader);
    pad_polls_to_limit(&mut expected);
    assert_eq!(
        serde_json::to_vec(&expected).unwrap().len(),
        MAX_RESPONSE_BYTES
    );
    let mut transaction = database.db.pool().begin().await.unwrap();
    for poll in &expected {
        insert_poll_raw(&mut transaction, poll, "2026-08-07T13:00:00.000Z").await;
    }
    transaction.commit().await.unwrap();
    expected.sort_by(|left, right| right.id.cmp(&left.id));
    let exact = polls
        .list_polls("trip-a", &human(&leader))
        .await
        .expect("exactly 1,000 polls and 4 MiB are accepted");
    assert_eq!(exact, expected);
    assert_eq!(
        serde_json::to_vec(&exact).unwrap().len(),
        MAX_RESPONSE_BYTES
    );

    let extendable = expected
        .iter()
        .find(|poll| poll.description.len() < 4_000)
        .unwrap();
    sqlx::query(
        "UPDATE polls SET description = description || 'x' \
         WHERE trip_id = 'trip-a' AND id = ?",
    )
    .bind(&extendable.id)
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        polls.list_polls("trip-a", &human(&leader)).await,
        Err(PollRepoError::SafetyLimitExceeded)
    );
    sqlx::query(
        "UPDATE polls SET description = substr(description, 1, length(description) - 1) \
         WHERE trip_id = 'trip-a' AND id = ?",
    )
    .bind(&extendable.id)
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        polls
            .create_decision_poll(
                "trip-a",
                &human(&leader),
                decision_poll("poll-overflow", &["overflow-a", "overflow-b"], false),
            )
            .await,
        Err(PollRepoError::SafetyLimitExceeded)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM polls WHERE trip_id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        1_000
    );
    let extra = Poll {
        id: "poll-1000".into(),
        trip_id: "trip-a".into(),
        created_by: leader.id.0.clone(),
        kind: PollKind::Decision,
        title: "Decision".into(),
        description: String::new(),
        options: vec![
            PollOption {
                id: "poll-1000-a".into(),
                label: "A".into(),
                proposal_id: None,
            },
            PollOption {
                id: "poll-1000-b".into(),
                label: "B".into(),
                proposal_id: None,
            },
        ],
        opens_at: None,
        closes_at: "2026-08-08T12:00:00.000Z".into(),
        decided_at: None,
        quorum: 1,
        allow_multi: false,
        status: PollStatus::Open,
        votes: Vec::new(),
        resolution_note: None,
    };
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_poll_raw(&mut transaction, &extra, "2026-08-07T13:00:00.000Z").await;
    transaction.commit().await.unwrap();
    assert_eq!(
        polls.list_polls("trip-a", &human(&leader)).await,
        Err(PollRepoError::SafetyLimitExceeded)
    );

    database.shutdown().await;
}

#[tokio::test]
async fn poll_byte_preflight_does_not_treat_repeated_storage_keys_as_wire_payload() {
    let database = TestDatabase::new().await;
    let (leader, _member, _viewer) = seed_graph(&database).await;
    let polls = database.polls();
    let mut transaction = database.db.pool().begin().await.unwrap();
    for index in 0..1_000 {
        let user_id = format!("voter-{index:04}");
        let email = format!("voter-{index:04}@example.com");
        sqlx::query(
            "INSERT INTO users (id, email, display_name, revision) \
             VALUES (?, ?, NULL, 1)",
        )
        .bind(&user_id)
        .bind(email)
        .execute(&mut *transaction)
        .await
        .unwrap();
    }
    for suffix in ['一', '二', '三', '四'] {
        let poll_id = format!("{}{suffix}", "界".repeat(199));
        let poll = Poll {
            id: poll_id.clone(),
            trip_id: "trip-a".into(),
            created_by: leader.id.0.clone(),
            kind: PollKind::Decision,
            title: "Storage-key boundary".into(),
            description: String::new(),
            options: vec![
                PollOption {
                    id: "a".into(),
                    label: "A".into(),
                    proposal_id: None,
                },
                PollOption {
                    id: "b".into(),
                    label: "B".into(),
                    proposal_id: None,
                },
            ],
            opens_at: None,
            closes_at: "2026-08-08T12:00:00.000Z".into(),
            decided_at: None,
            quorum: 1,
            allow_multi: false,
            status: PollStatus::Open,
            votes: Vec::new(),
            resolution_note: None,
        };
        insert_poll_raw(&mut transaction, &poll, "2026-08-07T13:00:00.000Z").await;
        for index in 0..1_000 {
            let user_id = format!("voter-{index:04}");
            sqlx::query(
                "INSERT INTO poll_ballots ( \
                     trip_id, poll_id, user_id, voted_at, revision \
                 ) VALUES ('trip-a', ?, ?, '2026-08-07T14:00:00.000Z', 1)",
            )
            .bind(&poll_id)
            .bind(&user_id)
            .execute(&mut *transaction)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO poll_ballot_options (trip_id, poll_id, user_id, option_id) \
                 VALUES ('trip-a', ?, ?, 'a')",
            )
            .bind(&poll_id)
            .bind(&user_id)
            .execute(&mut *transaction)
            .await
            .unwrap();
        }
    }
    transaction.commit().await.unwrap();

    let loaded = polls
        .list_polls("trip-a", &human(&leader))
        .await
        .expect("repeated normalized keys are not repeated response fields");
    assert_eq!(loaded.len(), 4);
    assert_eq!(
        loaded.iter().map(|poll| poll.votes.len()).sum::<usize>(),
        4_000
    );
    assert!(serde_json::to_vec(&loaded).unwrap().len() < MAX_RESPONSE_BYTES);

    database.shutdown().await;
}

#[tokio::test]
async fn concurrent_governance_writers_serialize_each_final_collection_slot() {
    let database = TestDatabase::new().await;
    let (_leader, member, _viewer) = seed_graph(&database).await;
    let proposals = database.proposals();
    let polls = database.polls();
    let mut transaction = database.db.pool().begin().await.unwrap();
    for proposal in boundary_proposals(&member).into_iter().take(999) {
        insert_proposal_raw(&mut transaction, &proposal).await;
    }
    for poll in boundary_polls(&member).into_iter().take(999) {
        insert_poll_raw(&mut transaction, &poll, "2026-08-07T13:00:00.000Z").await;
    }
    transaction.commit().await.unwrap();

    let proposal_barrier = Arc::new(Barrier::new(3));
    let mut proposal_tasks = Vec::new();
    for id in ["proposal-final-a", "proposal-final-b"] {
        let repository = proposals.clone();
        let actor = member.clone();
        let barrier = proposal_barrier.clone();
        proposal_tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            repository
                .create_proposal(
                    "trip-a",
                    &human(&actor),
                    add_stop_proposal(id, &actor, ProposalRoute::LeaderApproval),
                    application_ids(id),
                )
                .await
        }));
    }
    proposal_barrier.wait().await;
    let mut proposal_results = Vec::new();
    for task in proposal_tasks {
        proposal_results.push(task.await.unwrap());
    }
    assert_eq!(
        proposal_results
            .iter()
            .filter(|result| result.is_ok())
            .count(),
        1
    );
    assert_eq!(
        proposal_results
            .iter()
            .filter(|result| matches!(result, Err(ProposalRepoError::SafetyLimitExceeded)))
            .count(),
        1
    );

    let poll_barrier = Arc::new(Barrier::new(3));
    let mut poll_tasks = Vec::new();
    for (id, first, second) in [
        ("poll-final-a", "poll-final-a-1", "poll-final-a-2"),
        ("poll-final-b", "poll-final-b-1", "poll-final-b-2"),
    ] {
        let repository = polls.clone();
        let actor = member.clone();
        let barrier = poll_barrier.clone();
        poll_tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            repository
                .create_decision_poll(
                    "trip-a",
                    &human(&actor),
                    decision_poll(id, &[first, second], false),
                )
                .await
        }));
    }
    poll_barrier.wait().await;
    let mut poll_results = Vec::new();
    for task in poll_tasks {
        poll_results.push(task.await.unwrap());
    }
    assert_eq!(
        poll_results.iter().filter(|result| result.is_ok()).count(),
        1
    );
    assert_eq!(
        poll_results
            .iter()
            .filter(|result| matches!(result, Err(PollRepoError::SafetyLimitExceeded)))
            .count(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM proposals WHERE trip_id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        1_000
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM polls WHERE trip_id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        1_000
    );

    database.shutdown().await;
}

#[tokio::test]
async fn applied_change_sets_and_historical_rows_match_the_exact_published_transition() {
    let database = TestDatabase::new().await;
    let (leader, _member, _viewer) = seed_graph(&database).await;
    let proposals = database.proposals();
    let trips = database.trips();
    proposals
        .create_proposal(
            "trip-a",
            &human(&leader),
            add_stop_proposal(
                "proposal-transition-v2",
                &leader,
                ProposalRoute::LeaderApproval,
            ),
            application_ids("transition-v2"),
        )
        .await
        .unwrap();
    proposals
        .create_proposal(
            "trip-a",
            &human(&leader),
            day_change_proposal("proposal-transition-v3", &leader, 2, true),
            application_ids("transition-v3"),
        )
        .await
        .unwrap();

    let original_change_set: String = sqlx::query_scalar(
        "SELECT change_set_json FROM proposals \
         WHERE trip_id = 'trip-a' AND id = 'proposal-transition-v2'",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    let alternate = serde_json::to_string(&ChangeSet {
        base_plan_version: 1,
        ops: vec![ChangeOp::AddDay {
            date: "2026-08-10".into(),
            city_hint: "London".into(),
        }],
    })
    .unwrap();
    let mut transaction = database.db.pool().begin().await.unwrap();
    sqlx::query(
        "UPDATE proposals SET change_set_json = ? \
         WHERE trip_id = 'trip-a' AND id = 'proposal-transition-v2'",
    )
    .bind(&alternate)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE plans SET applied_change_set_json = ? \
         WHERE trip_id = 'trip-a' AND version = 2",
    )
    .bind(&alternate)
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    assert_eq!(
        trips.list_plan_versions("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    );
    assert_eq!(
        proposals.list_proposals("trip-a", &human(&leader)).await,
        Err(ProposalRepoError::CorruptData)
    );

    let mut transaction = database.db.pool().begin().await.unwrap();
    sqlx::query(
        "UPDATE proposals SET change_set_json = ? \
         WHERE trip_id = 'trip-a' AND id = 'proposal-transition-v2'",
    )
    .bind(&original_change_set)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE plans SET applied_change_set_json = ? \
         WHERE trip_id = 'trip-a' AND version = 2",
    )
    .bind(&original_change_set)
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    assert_eq!(
        trips
            .list_plan_versions("trip-a", &human(&leader))
            .await
            .unwrap()
            .len(),
        3
    );

    sqlx::query(
        "UPDATE plan_stops SET seq = 2.0 \
         WHERE trip_id = 'trip-a' AND plan_version = 2 \
           AND id = 'transition-v2-entity-0'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert_eq!(
        trips.list_plan_versions("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    );

    database.shutdown().await;
}

#[tokio::test]
async fn proposal_publication_enforces_exact_plan_history_and_concurrent_final_version() {
    let database = TestDatabase::new().await;
    let (leader, _member, _viewer) = seed_graph(&database).await;
    let proposals = database.proposals();
    let trips = database.trips();
    seed_plan_history(&database, &leader, 1_000).await;

    let exact = trips
        .list_plan_versions("trip-a", &human(&leader))
        .await
        .expect("exactly 1,000 reciprocal plan versions are accepted");
    assert_eq!(exact.len(), 1_000);
    assert_eq!(exact.last().unwrap().version, 1_000);
    assert_eq!(
        proposals
            .create_proposal(
                "trip-a",
                &human(&leader),
                day_change_proposal("history-overflow", &leader, 1_000, false),
                application_ids("history-overflow"),
            )
            .await,
        Err(ProposalRepoError::SafetyLimitExceeded)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM plans WHERE trip_id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        1_000
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM proposals \
             WHERE trip_id = 'trip-a' AND id = 'history-overflow'",
        )
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        0
    );

    let mut transaction = database.db.pool().begin().await.unwrap();
    append_plan_version_raw(&mut transaction, &leader, 1_001).await;
    sqlx::query(
        "UPDATE trips SET \
             current_plan_id = 'history-plan-1001', current_plan_version = 1001, \
             revision = revision + 1 \
         WHERE id = 'trip-a'",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    assert!(
        trips
            .list_plan_versions("trip-a", &human(&leader))
            .await
            .is_err()
    );
    database.shutdown().await;

    let database = TestDatabase::new().await;
    let (leader, _member, _viewer) = seed_graph(&database).await;
    let proposals = database.proposals();
    let trips = database.trips();
    seed_plan_history(&database, &leader, 999).await;
    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();
    for id in ["final-plan-a", "final-plan-b"] {
        let repository = proposals.clone();
        let actor = leader.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            repository
                .create_proposal(
                    "trip-a",
                    &human(&actor),
                    day_change_proposal(id, &actor, 999, true),
                    application_ids(id),
                )
                .await
        }));
    }
    barrier.wait().await;
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(ProposalRepoError::Conflict)))
            .count(),
        1
    );
    let versions = trips
        .list_plan_versions("trip-a", &human(&leader))
        .await
        .unwrap();
    assert_eq!(versions.len(), 1_000);
    assert_eq!(versions.last().unwrap().version, 1_000);

    database.shutdown().await;
}
