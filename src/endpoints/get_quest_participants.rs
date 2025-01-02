use crate::{models::AppState, utils::get_error};
use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};

use axum::http::StatusCode;
use axum_auto_routes::route;
use futures::StreamExt;
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize)]

pub struct GetQuestParticipantsQuery {
    quest_id: u32,
}

#[route(get, "/get_quest_participants")]
pub async fn handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<GetQuestParticipantsQuery>,
) -> impl IntoResponse {
    // Convert to int
    let quest_id = query.quest_id.to_string().parse::<i64>().unwrap();
    let tasks_collection = state.db.collection::<Document>("tasks");
    let tasks_ids = tasks_collection
        .find(doc! { "quest_id": quest_id }, None)
        .await
        .unwrap()
        .map(|task_doc| {
            task_doc
                .unwrap()
                .get("id")
                .unwrap()
                .to_string()
                .parse::<i64>()
                .unwrap()
        })
        .collect::<Vec<i64>>()
        .await;

    let tasks_count = tasks_ids.len();

    let pipeline = vec![
        doc! {
            "$match": {
                "task_id": {
                    "$in": tasks_ids
                }
            }
        },
        // First group by address to get the max timestamp for each participant
        doc! {
            "$group": {
                "_id": "$address",
                "count": { "$sum": 1 },
                "last_completion": { "$max": "$timestamp" }  // Get the timestamp of their last task
            }
        },
        // Filter for participants who completed all tasks
        doc! {
            "$match": {
                "count": tasks_count as i64
            }
        },
        // Sort by last completion time
        doc! {
            "$sort": {
                "last_completion": 1
            }
        },
        doc! {
            "$facet": {
                "total": [
                    { "$count": "count" }
                ],
                "participants": [
                    { "$limit": 3 },
                    { "$project": {
                        "address": "$_id",
                        "completion_time": "$last_completion",
                        "_id": 0
                    }}
                ]
            }
        },
        doc! {
            "$project": {
                "count": { "$ifNull": [{ "$arrayElemAt": ["$total.count", 0] }, 0] },
                "first_participants": "$participants"
            }
        },
    ];

    let completed_tasks_collection = state.db.collection::<Document>("completed_tasks");
    let mut cursor = completed_tasks_collection
        .aggregate(pipeline, None)
        .await
        .unwrap();

    let mut res: Document = Document::new();
    while let Some(result) = cursor.next().await {
        match result {
            Ok(document) => {
                res = document;
            }
            Err(_) => return get_error("Error querying quest participants".to_string()),
        }
    }

    (StatusCode::OK, Json(res)).into_response()
}
#[cfg(test)]
mod tests {
    use crate::{
        config::{self, Config},
        logger,
    };

    use super::*;
    use axum::body::HttpBody;
    use axum::{body::Bytes, http::StatusCode};
    use mongodb::{bson::doc, Client, Database};
    use reqwest::Url;
    use serde_json::Value;
    use starknet::providers::{jsonrpc::HttpTransport, JsonRpcClient};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    async fn setup_test_db() -> Database {
        let client = Client::with_uri_str("mongodb://localhost:27017")
            .await
            .expect("Failed to create MongoDB client");
        let db = client.database("test_db");

        // Clear collections before each test
        db.collection::<Document>("tasks").drop(None).await.ok();
        db.collection::<Document>("completed_tasks")
            .drop(None)
            .await
            .ok();

        db
    }

    async fn insert_test_data(db: Database, quest_id: i64, num_tasks: i64, num_participants: i64) {
        let tasks_collection = db.collection::<Document>("tasks");
        let completed_tasks_collection = db.collection::<Document>("completed_tasks");

        // Insert tasks
        for task_id in 1..=num_tasks {
            tasks_collection
                .insert_one(
                    doc! {
                        "id": task_id,
                        "quest_id": quest_id,
                    },
                    None,
                )
                .await
                .unwrap();
        }

        // Insert completed tasks for participants
        for participant in 1..=num_participants {
            let address = format!("participant_{}", participant);
            let base_timestamp = 1000 - (participant * 10); // Spaces out timestamps more clearly

            for task_id in 1..=num_tasks {
                completed_tasks_collection
                    .insert_one(
                        doc! {
                            "address": address.clone(),
                            "task_id": task_id,
                            // Last task for each participant will have the highest timestamp
                            "timestamp": base_timestamp + task_id
                        },
                        None,
                    )
                    .await
                    .unwrap();
            }
        }
    }

    #[tokio::test]
    async fn test_get_quest_participants() {
        // Setup
        let db = setup_test_db().await;
        let conf = config::load();
        let logger = logger::Logger::new(&conf.watchtower);
        let provider = JsonRpcClient::new(HttpTransport::new(
            Url::parse(&conf.variables.rpc_url).unwrap(),
        ));

        let app_state = Arc::new(AppState {
            db: db.clone(),
            last_task_id: Mutex::new(0),
            last_question_id: Mutex::new(0),
            conf,
            logger,
            provider,
        });

        // Test data
        let quest_id = 1;
        let num_tasks = 3;
        let num_participants = 5;

        insert_test_data(db.clone(), quest_id, num_tasks, num_participants).await;

        // Create request
        let query = GetQuestParticipantsQuery {
            quest_id: quest_id as u32,
        };

        // Execute request
        let response = handler(State(app_state), Query(query))
            .await
            .into_response();

        // Verify response
        assert_eq!(response.status(), StatusCode::OK);

        // Get the response body as bytes
        let body_bytes = match response.into_body().data().await {
            Some(Ok(bytes)) => bytes,
            _ => panic!("Failed to get response body"),
        };

        // Parse the body
        let body: Value = serde_json::from_slice(&body_bytes).unwrap();

        assert_eq!(body["count"], num_participants);
        assert_eq!(body["first_participants"].as_array().unwrap().len(), 3);
        println!("{:?}", body);

        // Verify first participants format
        let first_participants = body["first_participants"].as_array().unwrap();
        for participant in first_participants {
            assert!(participant.as_str().unwrap().starts_with("participant_"));
        }

        // Verify quest completion timestamp format
        let quest_completion_timestamp = body["first_participants"][0]["completion_time"]
            .as_i64()
            .unwrap();
        assert_eq!(quest_completion_timestamp, 973);
    }
}
