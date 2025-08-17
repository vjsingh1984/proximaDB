//! Comprehensive tests for routing module
//! Target: 80%+ coverage for request routing

use proximadb::core::routing::{Router, Route, RouteHandler, RoutingError};
use proximadb::core::VectorRecord;
use std::sync::Arc;
use std::collections::HashMap;

#[derive(Clone)]
struct MockHandler {
    name: String,
    calls: Arc<std::sync::Mutex<Vec<String>>>,
}

impl MockHandler {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            calls: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
    
    fn get_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl RouteHandler for MockHandler {
    type Request = String;
    type Response = String;
    type Error = RoutingError;
    
    async fn handle(&self, request: Self::Request) -> Result<Self::Response, Self::Error> {
        self.calls.lock().unwrap().push(request.clone());
        Ok(format!("{} handled: {}", self.name, request))
    }
}

#[tokio::test]
async fn test_router_creation() {
    let router: Router<String, String> = Router::new();
    
    // New router should have no routes
    assert_eq!(router.route_count(), 0);
}

#[tokio::test]
async fn test_router_add_route() {
    let mut router = Router::new();
    let handler = MockHandler::new("test_handler");
    
    // Add a simple route
    router.add_route("/api/test", Box::new(handler.clone()));
    assert_eq!(router.route_count(), 1);
    
    // Add another route
    router.add_route("/api/test2", Box::new(handler));
    assert_eq!(router.route_count(), 2);
}

#[tokio::test]
async fn test_router_exact_match() {
    let mut router = Router::new();
    let handler1 = MockHandler::new("handler1");
    let handler2 = MockHandler::new("handler2");
    
    router.add_route("/api/collections", Box::new(handler1));
    router.add_route("/api/vectors", Box::new(handler2));
    
    // Test exact matches
    let result = router.route("/api/collections", "request1".to_string()).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "handler1 handled: request1");
    
    let result = router.route("/api/vectors", "request2".to_string()).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "handler2 handled: request2");
}

#[tokio::test]
async fn test_router_wildcard_routes() {
    let mut router = Router::new();
    let handler = MockHandler::new("wildcard_handler");
    
    // Add wildcard route
    router.add_route("/api/collections/*", Box::new(handler.clone()));
    
    // Should match any path starting with /api/collections/
    let result = router.route("/api/collections/123", "req1".to_string()).await;
    assert!(result.is_ok());
    
    let result = router.route("/api/collections/123/vectors", "req2".to_string()).await;
    assert!(result.is_ok());
    
    // Should not match different prefix
    let result = router.route("/api/vectors/123", "req3".to_string()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_router_parameter_extraction() {
    let mut router = Router::new();
    let handler = MockHandler::new("param_handler");
    
    // Add parameterized route
    router.add_route("/api/collections/:id/vectors/:vector_id", Box::new(handler.clone()));
    
    // Test parameter extraction
    let result = router.route_with_params(
        "/api/collections/coll123/vectors/vec456",
        "request".to_string()
    ).await;
    
    assert!(result.is_ok());
    let (response, params) = result.unwrap();
    assert_eq!(response, "param_handler handled: request");
    assert_eq!(params.get(&key);
    assert_eq!(params.get(&key);
}

#[tokio::test]
async fn test_router_no_match() {
    let router: Router<String, String> = Router::new();
    
    // No routes registered
    let result = router.route("/api/test", "request".to_string()).await;
    assert!(result.is_err());
    
    match result.unwrap_err() {
        RoutingError::NoRouteFound(path) => assert_eq!(path, "/api/test"),
        _ => panic!("Expected NoRouteFound error"),
    }
}

#[tokio::test]
async fn test_router_priority() {
    let mut router = Router::new();
    let specific_handler = MockHandler::new("specific");
    let wildcard_handler = MockHandler::new("wildcard");
    
    // Add routes in different order to test priority
    router.add_route("/api/*", Box::new(wildcard_handler));
    router.add_route("/api/collections", Box::new(specific_handler));
    
    // Specific route should take precedence
    let result = router.route("/api/collections", "test".to_string()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("specific"));
    
    // Wildcard should catch others
    let result = router.route("/api/vectors", "test".to_string()).await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("wildcard"));
}

#[tokio::test]
async fn test_router_middleware() {
    let mut router = Router::new();
    let handler = MockHandler::new("main_handler");
    
    // Add route with middleware
    router.add_route_with_middleware(
        "/api/protected",
        Box::new(handler),
        vec![
            Box::new(|req: String| async move {
                // Auth middleware
                if req.contains("token") {
                    Ok(req)
                } else {
                    Err(RoutingError::Unauthorized)
                }
            }),
        ],
    );
    
    // Request without token should fail
    let result = router.route("/api/protected", "request".to_string()).await;
    assert!(result.is_err());
    
    // Request with token should succeed
    let result = router.route("/api/protected", "request with token".to_string()).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_route_builder() {
    let mut router = Router::new();
    
    // Use route builder pattern
    router
        .route("/api/v1")
        .get(key))
        .post("/collections", MockHandler::new("create_collection"))
        .get(key))
        .delete("/collections/:id", MockHandler::new("delete_collection"));
    
    assert!(router.route_count() >= 4);
}

#[tokio::test]
async fn test_concurrent_routing() {
    let router = Arc::new(Router::new());
    let handler = MockHandler::new("concurrent_handler");
    
    // Add route
    Arc::get_mut(&router).unwrap().add_route("/api/test", Box::new(handler.clone()));
    
    // Spawn multiple concurrent requests
    let mut handles = vec![];
    for i in 0..10 {
        let router_clone = router.clone();
        let handle = tokio::spawn(async move {
            router_clone.route("/api/test", format!("request{}", i)).await
        });
        handles.push(handle);
    }
    
    // All requests should succeed
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
    
    // Check all requests were handled
    assert_eq!(handler.get_calls().len(), 10);
}

#[tokio::test]
async fn test_route_groups() {
    let mut router = Router::new();
    
    // Create route groups
    router.group("/api/v1/collections", |group| {
        group
            .add("", MockHandler::new("list"))
            .add("/:id", MockHandler::new("get"))
            .add("/:id/vectors", MockHandler::new("list_vectors"));
    });
    
    // Test grouped routes
    assert!(router.route("/api/v1/collections", "test".to_string()).await.is_ok());
    assert!(router.route("/api/v1/collections/123", "test".to_string()).await.is_ok());
    assert!(router.route("/api/v1/collections/123/vectors", "test".to_string()).await.is_ok());
}

#[test]
fn test_route_pattern_matching() {
    let route = Route::new("/api/collections/:id/vectors/:vec_id");
    
    // Test exact match
    let params = route.match_path("/api/collections/123/vectors/456");
    assert!(params.is_some());
    let params = params.unwrap();
    assert_eq!(params.get(&key);
    assert_eq!(params.get(&key);
    
    // Test no match
    assert!(route.match_path("/api/collections/123").is_none());
    assert!(route.match_path("/api/vectors/123").is_none());
    
    // Test wildcard
    let wildcard_route = Route::new("/api/*");
    assert!(wildcard_route.match_path("/api/anything/goes/here").is_some());
    assert!(wildcard_route.match_path("/other/path").is_none());
}

#[test]
fn test_routing_error_types() {
    let err = RoutingError::NoRouteFound("/test".to_string());
    assert!(err.to_string().contains("No route found"));
    
    let err = RoutingError::InvalidPath("/test//double".to_string());
    assert!(err.to_string().contains("Invalid path"));
    
    let err = RoutingError::HandlerError("Custom error".to_string());
    assert!(err.to_string().contains("Custom error"));
    
    let err = RoutingError::Unauthorized;
    assert!(err.to_string().contains("Unauthorized"));
}