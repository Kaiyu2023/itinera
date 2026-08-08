use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ThreadAnchor {
    Trip,
    Day { day_id: String },
    Stop { stop_id: String },
    Poll { poll_id: String },
    Candidate { candidate_id: String },
}

impl<'de> Deserialize<'de> for ThreadAnchor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| D::Error::custom("thread anchor must be an object"))?;
        let kind = object
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| D::Error::custom("thread anchor kind must be a string"))?;
        let exact_fields = |expected: &[&str]| {
            object.len() == expected.len()
                && object
                    .keys()
                    .all(|field| expected.contains(&field.as_str()))
        };
        let id = |field: &str| {
            object
                .get(field)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| D::Error::custom(format!("thread anchor {field} must be a string")))
        };
        match kind {
            "trip" if exact_fields(&["kind"]) => Ok(Self::Trip),
            "day" if exact_fields(&["kind", "dayId"]) => Ok(Self::Day {
                day_id: id("dayId")?,
            }),
            "stop" if exact_fields(&["kind", "stopId"]) => Ok(Self::Stop {
                stop_id: id("stopId")?,
            }),
            "poll" if exact_fields(&["kind", "pollId"]) => Ok(Self::Poll {
                poll_id: id("pollId")?,
            }),
            "candidate" if exact_fields(&["kind", "candidateId"]) => Ok(Self::Candidate {
                candidate_id: id("candidateId")?,
            }),
            "trip" | "day" | "stop" | "poll" | "candidate" => Err(D::Error::custom(
                "thread anchor has unknown or missing fields",
            )),
            _ => Err(D::Error::custom("unsupported thread anchor kind")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscussionThread {
    pub id: String,
    pub trip_id: String,
    pub anchor: ThreadAnchor,
    pub title: String,
    pub comment_count: u32,
    pub last_activity_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reaction {
    pub emoji: String,
    pub user_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Comment {
    pub id: String,
    pub thread_id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub reactions: Vec<Reaction>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Comment, DiscussionThread, Reaction, ThreadAnchor};

    #[test]
    fn discussion_types_match_the_discriminated_wire_contract() {
        let thread = DiscussionThread {
            id: "thread-a".into(),
            trip_id: "trip-a".into(),
            anchor: ThreadAnchor::Stop {
                stop_id: "stop-a".into(),
            },
            title: "Dinner".into(),
            comment_count: 1,
            last_activity_at: "2026-08-06T10:00:00Z".into(),
        };
        assert_eq!(
            serde_json::to_value(&thread).expect("thread serializes"),
            json!({
                "id": "thread-a",
                "tripId": "trip-a",
                "anchor": { "kind": "stop", "stopId": "stop-a" },
                "title": "Dinner",
                "commentCount": 1,
                "lastActivityAt": "2026-08-06T10:00:00Z"
            })
        );

        let comment = Comment {
            id: "comment-a".into(),
            thread_id: "thread-a".into(),
            author: "user-a".into(),
            body: "Looks good".into(),
            created_at: "2026-08-06T10:00:00Z".into(),
            reactions: vec![Reaction {
                emoji: "👍".into(),
                user_ids: vec!["user-a".into()],
            }],
        };
        assert_eq!(
            serde_json::to_value(&comment).expect("comment serializes"),
            json!({
                "id": "comment-a",
                "threadId": "thread-a",
                "author": "user-a",
                "body": "Looks good",
                "createdAt": "2026-08-06T10:00:00Z",
                "reactions": [{ "emoji": "👍", "userIds": ["user-a"] }]
            })
        );

        assert!(
            serde_json::from_value::<ThreadAnchor>(json!({"kind": "trip", "stopId": "forged"}))
                .is_err()
        );
        assert!(serde_json::from_value::<ThreadAnchor>(json!({"kind": "stop"})).is_err());
    }
}
