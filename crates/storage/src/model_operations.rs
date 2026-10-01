use crate::{
    BoxFuture, StorageResult, model_agents::ModelPlanningConfiguration,
    model_execution::ModelExecutionConfiguration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelConfigurationStage {
    Planning,
    Execution,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "stage", content = "configuration", rename_all = "snake_case")]
pub enum ModelConfiguration {
    Planning(ModelPlanningConfiguration),
    Execution(Box<ModelExecutionConfiguration>),
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ModelConfigurationRecord {
    pub version: String,
    #[serde(flatten)]
    pub configuration: ModelConfiguration,
    pub valid_until_unix_ms: i64,
    pub disabled_at_unix_ms: Option<i64>,
    pub active: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ModelConfigurationPage {
    pub items: Vec<ModelConfigurationRecord>,
    pub next_cursor: Option<String>,
}

pub trait ModelOperationsStore: Send + Sync {
    fn list_model_configurations(
        &self,
        stage: ModelConfigurationStage,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ModelConfigurationPage>>;
    fn get_model_configuration(
        &self,
        stage: ModelConfigurationStage,
        version: &str,
    ) -> BoxFuture<'_, StorageResult<ModelConfigurationRecord>>;
}
