use crate::db::shows;
use crate::error::{AppError, AppResult};
use crate::services::omnivoice::{OmnivoiceClient, Voice};
use crate::state::AppState;
use crate::web::csrf::{self, CsrfToken};
use askama::Template;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, Response};
use axum::Extension;
use axum::Form;
use serde::Deserialize;

#[derive(Template)]
#[template(path = "partials/voice_picker.html")]
pub struct VoicePickerTemplate {
    pub csrf: String,
    pub show_id: i64,
    pub language: String,
    pub language_label: &'static str,
    pub voices: Vec<Voice>,
}

#[derive(Debug, Deserialize)]
pub struct VoiceForm {
    pub _csrf: String,
    pub voice_id: i64,
    pub voice_name: String,
}

pub fn validate_language(language: &str) -> AppResult<()> {
    if matches!(language, "vi" | "en" | "ko") {
        Ok(())
    } else {
        Err(AppError::BadRequest("Ngôn ngữ không được hỗ trợ.".into()))
    }
}

pub fn filter_ready_voices<'a>(voices: &'a [Voice], language: &str) -> Vec<&'a Voice> {
    voices
        .iter()
        .filter(|voice| voice.status == "ready" && voice.language.as_deref() == Some(language))
        .collect()
}

fn language_label(language: &str) -> &'static str {
    match language {
        "vi" => "Tiếng Việt",
        "en" => "English",
        "ko" => "한국어",
        _ => "Không xác định",
    }
}

pub async fn picker(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path((show_id, language)): Path<(i64, String)>,
) -> AppResult<Html<String>> {
    validate_language(&language)?;
    shows::get(&state.pool, show_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let voices = client
        .list_voices()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let voices = filter_ready_voices(&voices, &language)
        .into_iter()
        .cloned()
        .collect();
    Ok(Html(
        VoicePickerTemplate {
            csrf: token.0,
            show_id,
            language_label: language_label(&language),
            language,
            voices,
        }
        .render()?,
    ))
}

pub async fn select(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path((show_id, language)): Path<(i64, String)>,
    Form(form): Form<VoiceForm>,
) -> AppResult<Response> {
    csrf::verify(&token, &headers, &form._csrf)?;
    validate_language(&language)?;
    shows::get(&state.pool, show_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let voices = client
        .list_voices()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let selected = voices
        .iter()
        .find(|voice| {
            voice.id == form.voice_id
                && voice.status == "ready"
                && voice.language.as_deref() == Some(language.as_str())
        })
        .ok_or_else(|| AppError::BadRequest("Giọng đọc không còn khả dụng.".into()))?;
    let name = selected.name.clone().unwrap_or(form.voice_name);
    shows::upsert_voice(&state.pool, show_id, &language, selected.id, &name).await?;
    Ok(crate::web::shows::redirect_response(
        &headers,
        &format!("/shows/{show_id}?tab=voices"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::omnivoice::Voice;

    fn voice(id: i64, language: Option<&str>, status: &str) -> Voice {
        Voice {
            id,
            name: Some(format!("Voice {id}")),
            status: status.into(),
            language: language.map(str::to_string),
        }
    }

    #[test]
    fn filters_to_ready_voices_in_the_requested_language() {
        let voices = vec![
            voice(1, Some("vi"), "ready"),
            voice(2, Some("vi"), "failed"),
            voice(3, Some("en"), "ready"),
            voice(4, None, "ready"),
        ];
        let filtered = filter_ready_voices(&voices, "vi");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, 1);
    }

    #[test]
    fn reject_unsupported_language_before_network() {
        assert!(validate_language("ja").is_err());
        assert!(validate_language("vi").is_ok());
    }
}
