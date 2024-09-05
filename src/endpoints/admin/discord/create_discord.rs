use crate::models::{QuestDocument,QuestTaskDocument};
use crate::utils::verify_quest_auth;
use crate::{models::AppState, utils::get_error};
use crate::middleware::auth::auth_middleware;
use axum::{
    extract::{State, Extension},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use axum_auto_routes::route;
use mongodb::bson::doc;
use mongodb::options::FindOneOptions;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub_struct!(Deserialize; CreateCustom {
    quest_id: i64,
    name: String,
    desc: String,
    invite_link: String,
    guild_id: String,
});

#[route(post, "/admin/tasks/discord/create", auth_middleware)]
pub async fn handler(
    State(state): State<Arc<AppState>>,
    Extension(sub): Extension<String>,
    Json(body): Json<CreateCustom>,
) -> impl IntoResponse {
    let collection = state.db.collection::<QuestTaskDocument>("tasks");
    // Get the last id in increasing order
    let last_id_filter = doc! {};
    let options = FindOneOptions::builder().sort(doc! {"id": -1}).build();
    let last_doc = &collection.find_one(last_id_filter, options).await.unwrap();

    let quests_collection = state.db.collection::<QuestDocument>("quests");

    let res = verify_quest_auth(sub, &quests_collection, &(body.quest_id as i64)).await;
    if !res {
        return get_error("Error creating task".to_string());
    };

    let mut next_id = 1;
    if let Some(doc) = last_doc {
        let last_id = doc.id;
        next_id = last_id + 1;
    }

    let new_document = QuestTaskDocument {
        name: body.name.clone(),
        desc: body.desc.clone(),
        href: body.invite_link.clone(),
        quest_id: body.quest_id.clone(),
        id: next_id,
        total_amount: None,
        cta: "Join now!".to_string(),
        verify_endpoint: "quests/discord_fw_callback".to_string(),
        verify_endpoint_type: "oauth_discord".to_string(),
        task_type: Some("discord".to_string()),
        discord_guild_id: Some(body.guild_id.clone()),
        quiz_name: None,
        verify_redirect: None,
        contracts: None,
    };

    // insert document to boost collection
    return match collection.insert_one(new_document, None).await {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({"message": "Task created successfully"})).into_response(),
        )
            .into_response(),
        Err(_e) => get_error("Error creating task".to_string()),
    };
}
