use crate::{PostgresStore, map_error};
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    model_operations::{
        ModelConfiguration, ModelConfigurationPage, ModelConfigurationRecord,
        ModelConfigurationStage, ModelOperationsStore,
    },
    reply_operations::validate_reply_revision,
};
use sqlx::{Row, postgres::PgRow};

fn table(stage: ModelConfigurationStage) -> &'static str {
    match stage {
        ModelConfigurationStage::Planning => "model_planning_configurations",
        ModelConfigurationStage::Execution => "model_execution_configurations",
    }
}

const FIELDS: &str = "version,configuration::text AS config,floor(extract(epoch FROM disabled_at)*1000)::bigint AS disabled_ms,floor(extract(epoch FROM statement_timestamp())*1000)::bigint AS now_ms";

fn record(stage: ModelConfigurationStage, row: &PgRow) -> StorageResult<ModelConfigurationRecord> {
    let invalid = |_| StorageError::Unavailable("invalid stored model configuration".into());
    let config: &str = row.get("config");
    let configuration = match stage {
        ModelConfigurationStage::Planning => {
            ModelConfiguration::Planning(serde_json::from_str(config).map_err(invalid)?)
        }
        ModelConfigurationStage::Execution => {
            ModelConfiguration::Execution(serde_json::from_str(config).map_err(invalid)?)
        }
    };
    let valid_until_unix_ms = match &configuration {
        ModelConfiguration::Planning(c) => c.budget.valid_until_unix_ms,
        ModelConfiguration::Execution(c) => c
            .embedding
            .valid_until_unix_ms
            .min(c.answer.valid_until_unix_ms),
    };
    let disabled_at_unix_ms = row.get("disabled_ms");
    Ok(ModelConfigurationRecord {
        version: row.get("version"),
        configuration,
        valid_until_unix_ms,
        disabled_at_unix_ms,
        active: disabled_at_unix_ms.is_none() && valid_until_unix_ms > row.get::<i64, _>("now_ms"),
    })
}

impl ModelOperationsStore for PostgresStore {
    fn list_model_configurations(
        &self,
        stage: ModelConfigurationStage,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ModelConfigurationPage>> {
        let after = after.map(str::to_owned);
        Box::pin(async move {
            if let Some(after) = &after {
                validate_reply_revision(after)?;
            }
            let rows = sqlx::query(&format!("SELECT {FIELDS} FROM {} WHERE ($1::text IS NULL OR version>$1) ORDER BY version LIMIT 101", table(stage)))
                .bind(after).fetch_all(&self.pool).await.map_err(map_error)?;
            let items = rows
                .iter()
                .take(100)
                .map(|row| record(stage, row))
                .collect::<StorageResult<Vec<_>>>()?;
            let next_cursor = (rows.len() > 100).then(|| {
                items
                    .last()
                    .expect("full configuration page")
                    .version
                    .clone()
            });
            Ok(ModelConfigurationPage { items, next_cursor })
        })
    }

    fn get_model_configuration(
        &self,
        stage: ModelConfigurationStage,
        version: &str,
    ) -> BoxFuture<'_, StorageResult<ModelConfigurationRecord>> {
        let version = version.to_owned();
        Box::pin(async move {
            validate_reply_revision(&version)?;
            let row = sqlx::query(&format!(
                "SELECT {FIELDS} FROM {} WHERE version=$1",
                table(stage)
            ))
            .bind(version)
            .fetch_one(&self.pool)
            .await
            .map_err(map_error)?;
            record(stage, &row)
        })
    }
}
