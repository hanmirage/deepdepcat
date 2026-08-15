use super::*;

#[test]
fn test_subagent_type_read_only() {
    assert!(!SubagentType::General.is_read_only());
    assert!(SubagentType::Explore.is_read_only());
    assert!(SubagentType::Plan.is_read_only());
    assert!(!SubagentType::Custom("test".to_string()).is_read_only());
}

#[test]
fn test_subagent_type_as_str() {
    assert_eq!(SubagentType::General.as_str(), "general");
    assert_eq!(SubagentType::Explore.as_str(), "explore");
    assert_eq!(SubagentType::Plan.as_str(), "plan");
    assert_eq!(
        SubagentType::Custom("custom".to_string()).as_str(),
        "custom"
    );
}

#[test]
fn test_subagent_config_default() {
    let config = SubagentConfig::default();
    assert!(matches!(config.agent_type, SubagentType::General));
    assert_eq!(config.max_turns, 20);
    assert_eq!(config.depth, 0);
    assert!(config.surface_completion);
    assert!(!config.fork);
    assert!(config.fork_context.is_empty());
}

#[test]
fn test_worker_definition_serialization() {
    let worker = WorkerDefinition {
        name: "explorer".to_string(),
        task: "Find all TODO comments".to_string(),
        agent_type: SubagentType::Explore,
        model: None,
        max_turns: 10,
        paths: None,
    };

    let json = serde_json::to_string(&worker).unwrap();
    let decoded: WorkerDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.name, "explorer");
    assert_eq!(decoded.max_turns, 10);
}
