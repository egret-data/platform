//! Native Trino monitor DTOs + pure Trino-REST-JSON → DTO mappers.
//!
//! These functions map the coordinator's `/v1/query`, `/v1/query/{id}`, and
//! `/v1/node` JSON into egret DTOs serialized to the frontend. Times/sizes pass
//! through as RAW Trino strings ("1.23s", "12MB") — no Duration/DataSize
//! parsing — and `outputStage` passes through as a raw `serde_json::Value` for
//! the frontend stage visualizer.

use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QueryDto {
    pub id: String,
    pub user: String,
    pub state: String,
    pub query: String,
    pub elapsed: String,
    pub cpu: String,
    pub memory: String,
    pub progress_percent: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QueryDetailDto {
    #[serde(flatten)]
    pub base: QueryDto,
    pub error_message: Option<String>,
    pub output_stage: Option<Value>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NodeDto {
    pub uri: String,
    pub recent_failures: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClusterDto {
    pub running: u32,
    pub queued: u32,
    pub blocked: u32,
    pub nodes: Vec<NodeDto>,
}

fn s(v: &Value, p: &str) -> String {
    v.get(p).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

pub fn query_dto(v: &Value) -> QueryDto {
    let qs = &v["queryStats"];
    QueryDto {
        id: s(v, "queryId"),
        user: v["session"]["user"].as_str().unwrap_or("").to_string(),
        state: s(v, "state"),
        query: s(v, "query"),
        elapsed: qs["elapsedTime"].as_str().unwrap_or("").to_string(),
        cpu: qs["totalCpuTime"].as_str().unwrap_or("").to_string(),
        memory: qs["userMemoryReservation"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        progress_percent: qs["progressPercentage"].as_f64().unwrap_or(0.0),
    }
}

pub fn query_detail_dto(v: &Value) -> QueryDetailDto {
    QueryDetailDto {
        base: query_dto(v),
        error_message: v["failureInfo"]["message"].as_str().map(|s| s.to_string()),
        output_stage: v.get("outputStage").filter(|s| !s.is_null()).cloned(),
    }
}

pub fn cluster_dto(queries: &Value, nodes: &Value) -> ClusterDto {
    let count = |st: &str| {
        queries
            .as_array()
            .map(|a| a.iter().filter(|q| q["state"] == st).count() as u32)
            .unwrap_or(0)
    };
    ClusterDto {
        running: count("RUNNING"),
        queued: count("QUEUED"),
        blocked: count("BLOCKED"),
        nodes: nodes
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|n| NodeDto {
                        uri: n["uri"].as_str().unwrap_or("").to_string(),
                        recent_failures: n["recentFailures"].as_f64().unwrap_or(0.0),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn map_query_list_item() {
        let v = json!({"queryId":"20260704_x","state":"RUNNING","query":"SELECT 1",
            "session":{"user":"bob"},
            "queryStats":{"elapsedTime":"1.23s","totalCpuTime":"0.50s","userMemoryReservation":"12MB","progressPercentage":42.0}});
        let d = query_dto(&v);
        assert_eq!(d.id, "20260704_x");
        assert_eq!(d.user, "bob");
        assert_eq!(d.state, "RUNNING");
        assert_eq!(d.query, "SELECT 1");
        assert_eq!(d.elapsed, "1.23s");
        assert_eq!(d.memory, "12MB");
        assert_eq!(d.progress_percent, 42.0);
    }

    #[test]
    fn map_query_detail_passes_output_stage_and_error() {
        let v = json!({"queryId":"q","state":"FAILED","query":"SELECT 1","session":{"user":"u"},
            "queryStats":{"elapsedTime":"1s","totalCpuTime":"1s","userMemoryReservation":"0B","progressPercentage":100.0},
            "failureInfo":{"message":"boom"},
            "outputStage":{"stageId":"0","state":"FINISHED","subStages":[]}});
        let d = query_detail_dto(&v);
        assert_eq!(d.base.state, "FAILED");
        assert_eq!(d.error_message.as_deref(), Some("boom"));
        assert_eq!(d.output_stage.as_ref().unwrap()["stageId"], "0");
    }

    #[test]
    fn cluster_tally_counts_states() {
        let queries = json!([{"state":"RUNNING"},{"state":"RUNNING"},{"state":"QUEUED"},{"state":"FINISHED"}]);
        let nodes = json!([{"uri":"http://w1:8080","recentFailures":0.0}]);
        let c = cluster_dto(&queries, &nodes);
        assert_eq!(c.running, 2);
        assert_eq!(c.queued, 1);
        assert_eq!(c.nodes.len(), 1);
    }
}
