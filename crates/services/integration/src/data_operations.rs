use std::collections::HashSet;

use async_graphql::Result;
use common_utils::ryot_log;
use database_models::{integration, prelude::Integration};
use database_utils::user_by_id;

use dependent_utils::{get_google_books_service, get_hardcover_service, get_openlibrary_service};
use enum_models::{IntegrationLot, IntegrationProvider};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use traits::TraceOk;

use crate::integration_management::IntegrationManager;
use crate::{IntegrationService, yank};

impl IntegrationService {
    async fn yank_integrations_data_for_user(&self, user_id: &String) -> Result<()> {
        let preferences = user_by_id(user_id, &self.0).await?.preferences;
        if preferences.general.disable_integrations {
            return Ok(());
        }
        let integrations = IntegrationManager::select_integrations_to_process(
            &self.0,
            user_id,
            IntegrationLot::Yank,
            None,
        )
        .await?;
        let mut progress_updates = vec![];
        for integration in integrations.into_iter() {
            let specifics = integration.clone().provider_specifics.unwrap();
            let response = match integration.provider {
                IntegrationProvider::Audiobookshelf => {
                    yank::audiobookshelf::yank_progress(
                        specifics.audiobookshelf_base_url.unwrap(),
                        specifics.audiobookshelf_token.unwrap(),
                        &self.0,
                        &get_hardcover_service(&self.0.config).await.unwrap(),
                        &get_google_books_service(&self.0.config).await.unwrap(),
                        &get_openlibrary_service(&self.0.config).await.unwrap(),
                    )
                    .await
                }
                IntegrationProvider::Komga => {
                    yank::komga::yank_progress(
                        specifics.komga_base_url.unwrap(),
                        specifics.komga_username.unwrap(),
                        specifics.komga_password.unwrap(),
                        specifics.komga_provider.unwrap(),
                        &self.0.db,
                    )
                    .await
                }
                IntegrationProvider::YoutubeMusic => {
                    database_utils::server_key_validation_guard(
                        self.0.is_server_key_validated().await?,
                    )
                    .await?;
                    yank::youtube_music::yank_progress(
                        user_id,
                        specifics.youtube_music_timezone.unwrap(),
                        specifics.youtube_music_auth_cookie.unwrap(),
                        &self.0,
                    )
                    .await
                }
                _ => continue,
            };
            match response {
                Ok(update) => progress_updates.push((integration, update)),
                Err(e) => {
                    IntegrationManager::set_trigger_result(
                        &self.0,
                        Some(e.to_string()),
                        &integration,
                    )
                    .await?;
                }
            };
        }
        for (integration, progress_updates) in progress_updates.into_iter() {
            self.integration_progress_update(integration, progress_updates)
                .await
                .trace_ok();
        }
        Ok(())
    }

    pub async fn yank_integrations_data(&self) -> Result<()> {
        let users_with_integrations = Integration::find()
            .filter(integration::Column::Lot.eq(IntegrationLot::Yank))
            .select_only()
            .column(integration::Column::UserId)
            .into_tuple::<String>()
            .all(&self.0.db)
            .await?
            .into_iter()
            .collect::<HashSet<String>>();
        for user_id in users_with_integrations {
            ryot_log!(debug, "Yanking integrations data for user {}", user_id);
            self.yank_integrations_data_for_user(&user_id).await?;
        }
        Ok(())
    }

    async fn sync_integrations_data_to_owned_collection_for_user(
        &self,
        user_id: &String,
    ) -> Result<bool> {
        let preferences = user_by_id(user_id, &self.0).await?.preferences;
        if preferences.general.disable_integrations {
            return Ok(false);
        }
        let integrations = IntegrationManager::select_integrations_to_process(
            &self.0,
            user_id,
            IntegrationLot::Yank,
            None,
        )
        .await?;
        let mut progress_updates = vec![];
        for integration in integrations.into_iter() {
            if !integration.sync_to_owned_collection.unwrap_or_default() {
                continue;
            }
            let specifics = integration.clone().provider_specifics.unwrap();
            let response = match integration.provider {
                IntegrationProvider::Audiobookshelf => {
                    yank::audiobookshelf::sync_to_owned_collection(
                        specifics.audiobookshelf_base_url.unwrap(),
                        &get_hardcover_service(&self.0.config).await.unwrap(),
                        &get_google_books_service(&self.0.config).await.unwrap(),
                        &get_openlibrary_service(&self.0.config).await.unwrap(),
                    )
                    .await
                }
                IntegrationProvider::Komga => {
                    yank::komga::sync_to_owned_collection(
                        specifics.komga_base_url.unwrap(),
                        specifics.komga_username.unwrap(),
                        specifics.komga_password.unwrap(),
                        specifics.komga_provider.unwrap(),
                        &self.0.db,
                    )
                    .await
                }
                IntegrationProvider::PlexYank => {
                    yank::plex::sync_to_owned_collection(
                        specifics.plex_yank_base_url.unwrap(),
                        specifics.plex_yank_token.unwrap(),
                    )
                    .await
                }
                _ => continue,
            };
            match response {
                Ok(update) => progress_updates.push((integration, update)),
                Err(e) => {
                    IntegrationManager::set_trigger_result(
                        &self.0,
                        Some(e.to_string()),
                        &integration,
                    )
                    .await?;
                }
            };
        }
        for (integration, progress_updates) in progress_updates.into_iter() {
            self.integration_progress_update(integration, progress_updates)
                .await
                .trace_ok();
        }
        Ok(true)
    }

    async fn sync_integrations_data_to_owned_collection(&self) -> Result<()> {
        let users_with_integrations = Integration::find()
            .filter(integration::Column::SyncToOwnedCollection.eq(true))
            .select_only()
            .column(integration::Column::UserId)
            .into_tuple::<String>()
            .all(&self.0.db)
            .await?
            .into_iter()
            .collect::<HashSet<String>>();
        for user_id in users_with_integrations {
            ryot_log!(
                debug,
                "Syncing integrations data to owned collection for user {}",
                user_id
            );
            self.sync_integrations_data_to_owned_collection_for_user(&user_id)
                .await?;
        }
        Ok(())
    }

    pub async fn sync_integrations_data_for_user(&self, user_id: &String) -> Result<()> {
        self.sync_integrations_data_to_owned_collection_for_user(user_id)
            .await?;
        self.yank_integrations_data_for_user(user_id).await?;
        Ok(())
    }

    pub async fn sync_integrations_data(&self) -> Result<()> {
        self.yank_integrations_data().await?;
        self.sync_integrations_data_to_owned_collection().await?;
        Ok(())
    }
}
