"""
Multi-Language Code Indexing Integration Tests.

Tests code indexing and semantic search across all supported languages:
- Python, JavaScript/TypeScript, Rust, Go, Java, C++
- Ruby, C#, PHP, Swift, Kotlin, Scala
- Bash, SQL, YAML, JSON, XML
- Perl, Lua, Haskell, Elixir

Real-world code samples test semantic search effectiveness for:
- Finding similar functions across languages
- Code relationship detection
- Documentation-based search
- Pattern matching (e.g., "error handling", "async operations")

Requirements:
- Running ProximaDB server at localhost:5678
"""

import hashlib
import time
from dataclasses import dataclass
from typing import Any, Dict, List, Optional

import pytest
import requests

# =============================================================================
# Test Configuration
# =============================================================================

SERVER_URL = "http://localhost:5678"


def is_server_available() -> bool:
    """Check if ProximaDB server is running."""
    try:
        response = requests.get(f"{SERVER_URL}/health", timeout=5)
        return response.status_code == 200
    except Exception:
        return False


pytestmark = pytest.mark.skipif(
    not is_server_available(), reason="ProximaDB server not available at localhost:5678"
)


# =============================================================================
# Real-World Code Samples for Each Language
# =============================================================================

LANGUAGE_SAMPLES = {
    "python": {
        "extension": ".py",
        "samples": [
            {
                "id": "py_async_http",
                "name": "fetch_data",
                "type": "function",
                "description": "Asynchronous HTTP client for fetching data from REST API",
                "code": '''
async def fetch_data(url: str, timeout: int = 30) -> dict:
    """Fetch JSON data from a REST API endpoint asynchronously.

    Handles connection errors, timeouts, and retries with exponential backoff.

    Args:
        url: The API endpoint URL
        timeout: Request timeout in seconds

    Returns:
        Parsed JSON response as dictionary

    Raises:
        ConnectionError: If unable to connect after retries
        TimeoutError: If request exceeds timeout
    """
    async with aiohttp.ClientSession() as session:
        for attempt in range(3):
            try:
                async with session.get(url, timeout=timeout) as response:
                    response.raise_for_status()
                    return await response.json()
            except aiohttp.ClientError as e:
                if attempt == 2:
                    raise ConnectionError(f"Failed to fetch {url}") from e
                await asyncio.sleep(2 ** attempt)
''',
            },
            {
                "id": "py_cache_decorator",
                "name": "memoize",
                "type": "function",
                "description": "Caching decorator with TTL support for function results",
                "code": '''
def memoize(ttl_seconds: int = 300):
    """Decorator that caches function results with time-to-live.

    Uses LRU cache with TTL expiration for memory-efficient caching.
    Thread-safe implementation using locks.

    Args:
        ttl_seconds: Cache entry lifetime in seconds

    Returns:
        Decorated function with caching behavior
    """
    def decorator(func):
        cache = {}
        lock = threading.Lock()

        @functools.wraps(func)
        def wrapper(*args, **kwargs):
            key = (args, tuple(sorted(kwargs.items())))
            now = time.time()

            with lock:
                if key in cache:
                    result, timestamp = cache[key]
                    if now - timestamp < ttl_seconds:
                        return result

            result = func(*args, **kwargs)
            with lock:
                cache[key] = (result, now)
            return result
        return wrapper
    return decorator
''',
            },
            {
                "id": "py_db_repository",
                "name": "UserRepository",
                "type": "class",
                "description": "Database repository pattern for user CRUD operations",
                "code": '''
class UserRepository:
    """Repository for user data access with connection pooling.

    Implements the repository pattern for clean separation of data access.
    Supports transactions, pagination, and soft deletes.
    """

    def __init__(self, connection_pool: ConnectionPool):
        self.pool = connection_pool
        self.table = "users"

    async def find_by_id(self, user_id: int) -> Optional[User]:
        """Find user by primary key."""
        async with self.pool.acquire() as conn:
            row = await conn.fetchone(
                f"SELECT * FROM {self.table} WHERE id = ? AND deleted_at IS NULL",
                (user_id,)
            )
            return User.from_row(row) if row else None

    async def create(self, user: User) -> User:
        """Create new user record."""
        async with self.pool.acquire() as conn:
            result = await conn.execute(
                f"INSERT INTO {self.table} (name, email, password_hash) VALUES (?, ?, ?)",
                (user.name, user.email, user.password_hash)
            )
            user.id = result.lastrowid
            return user
''',
            },
        ],
    },
    "javascript": {
        "extension": ".js",
        "samples": [
            {
                "id": "js_promise_queue",
                "name": "PromiseQueue",
                "type": "class",
                "description": "Promise-based task queue with concurrency control",
                "code": """
class PromiseQueue {
    /**
     * Queue for managing concurrent promise execution.
     *
     * Limits concurrent operations to prevent resource exhaustion.
     * Supports priority queuing and cancellation.
     */
    constructor(concurrency = 4) {
        this.concurrency = concurrency;
        this.pending = [];
        this.running = 0;
    }

    async add(fn, priority = 0) {
        return new Promise((resolve, reject) => {
            const task = { fn, resolve, reject, priority };
            this.pending.push(task);
            this.pending.sort((a, b) => b.priority - a.priority);
            this.process();
        });
    }

    async process() {
        while (this.running < this.concurrency && this.pending.length > 0) {
            const task = this.pending.shift();
            this.running++;

            try {
                const result = await task.fn();
                task.resolve(result);
            } catch (error) {
                task.reject(error);
            } finally {
                this.running--;
                this.process();
            }
        }
    }
}
""",
            },
            {
                "id": "js_event_emitter",
                "name": "EventEmitter",
                "type": "class",
                "description": "Custom event emitter with once and wildcard support",
                "code": """
class EventEmitter {
    /**
     * Lightweight event emitter implementation.
     *
     * Supports: on, once, off, emit, wildcard listeners (*).
     * Memory-safe with automatic listener cleanup.
     */
    constructor() {
        this.listeners = new Map();
    }

    on(event, callback) {
        if (!this.listeners.has(event)) {
            this.listeners.set(event, new Set());
        }
        this.listeners.get(event).add(callback);
        return () => this.off(event, callback);
    }

    once(event, callback) {
        const wrapper = (...args) => {
            this.off(event, wrapper);
            callback(...args);
        };
        return this.on(event, wrapper);
    }

    emit(event, ...args) {
        const handlers = this.listeners.get(event) || new Set();
        const wildcards = this.listeners.get('*') || new Set();

        handlers.forEach(fn => fn(...args));
        wildcards.forEach(fn => fn(event, ...args));
    }
}
""",
            },
        ],
    },
    "rust": {
        "extension": ".rs",
        "samples": [
            {
                "id": "rs_result_ext",
                "name": "ResultExt",
                "type": "trait",
                "description": "Extension trait for Result with additional error handling methods",
                "code": """
/// Extension trait providing additional methods for Result types.
///
/// Adds context, retry, and logging capabilities to standard Results.
pub trait ResultExt<T, E> {
    /// Add context to an error for better debugging.
    fn context<C: Into<String>>(self, ctx: C) -> Result<T, ContextError<E>>;

    /// Retry the operation with exponential backoff.
    fn retry(self, attempts: usize) -> Result<T, E>
    where
        Self: Sized + Clone;

    /// Log the error if present, returning the original Result.
    fn log_error(self, level: LogLevel) -> Self;
}

impl<T, E: std::error::Error> ResultExt<T, E> for Result<T, E> {
    fn context<C: Into<String>>(self, ctx: C) -> Result<T, ContextError<E>> {
        self.map_err(|e| ContextError {
            context: ctx.into(),
            source: e,
        })
    }

    fn retry(self, attempts: usize) -> Result<T, E> {
        // Implementation with exponential backoff
        todo!()
    }

    fn log_error(self, level: LogLevel) -> Self {
        if let Err(ref e) = self {
            log::log!(level, "Error: {}", e);
        }
        self
    }
}
""",
            },
            {
                "id": "rs_async_pool",
                "name": "ConnectionPool",
                "type": "struct",
                "description": "Async connection pool with health checking and automatic reconnection",
                "code": """
/// Async connection pool for database connections.
///
/// Features:
/// - Configurable pool size
/// - Health checking with automatic reconnection
/// - Fair connection distribution
/// - Metrics and monitoring
pub struct ConnectionPool<C: Connection> {
    connections: Arc<Mutex<VecDeque<C>>>,
    config: PoolConfig,
    health_checker: Arc<dyn HealthChecker<C>>,
    metrics: PoolMetrics,
}

impl<C: Connection + Send + 'static> ConnectionPool<C> {
    /// Create a new connection pool with the given configuration.
    pub async fn new(config: PoolConfig) -> Result<Self, PoolError> {
        let mut connections = VecDeque::with_capacity(config.max_size);

        for _ in 0..config.min_size {
            let conn = C::connect(&config.connection_string).await?;
            connections.push_back(conn);
        }

        Ok(Self {
            connections: Arc::new(Mutex::new(connections)),
            config,
            health_checker: Arc::new(DefaultHealthChecker),
            metrics: PoolMetrics::default(),
        })
    }

    /// Acquire a connection from the pool.
    pub async fn acquire(&self) -> Result<PooledConnection<C>, PoolError> {
        let timeout = Duration::from_secs(self.config.acquire_timeout);
        tokio::time::timeout(timeout, self.acquire_inner()).await?
    }
}
""",
            },
        ],
    },
    "go": {
        "extension": ".go",
        "samples": [
            {
                "id": "go_worker_pool",
                "name": "WorkerPool",
                "type": "struct",
                "description": "Concurrent worker pool with graceful shutdown",
                "code": """
// WorkerPool manages a pool of goroutines for concurrent task processing.
//
// Features:
// - Configurable worker count
// - Graceful shutdown with context
// - Panic recovery per worker
// - Metrics collection
type WorkerPool struct {
    workers   int
    taskQueue chan Task
    results   chan Result
    wg        sync.WaitGroup
    ctx       context.Context
    cancel    context.CancelFunc
}

// NewWorkerPool creates a new worker pool with the specified number of workers.
func NewWorkerPool(workers int, queueSize int) *WorkerPool {
    ctx, cancel := context.WithCancel(context.Background())
    return &WorkerPool{
        workers:   workers,
        taskQueue: make(chan Task, queueSize),
        results:   make(chan Result, queueSize),
        ctx:       ctx,
        cancel:    cancel,
    }
}

// Start begins processing tasks with the worker pool.
func (p *WorkerPool) Start() {
    for i := 0; i < p.workers; i++ {
        p.wg.Add(1)
        go p.worker(i)
    }
}

// worker processes tasks from the queue.
func (p *WorkerPool) worker(id int) {
    defer p.wg.Done()
    defer func() {
        if r := recover(); r != nil {
            log.Printf("Worker %d panicked: %v", id, r)
        }
    }()

    for {
        select {
        case <-p.ctx.Done():
            return
        case task := <-p.taskQueue:
            result := task.Execute()
            p.results <- result
        }
    }
}
""",
            },
            {
                "id": "go_middleware",
                "name": "LoggingMiddleware",
                "type": "function",
                "description": "HTTP middleware for request logging with structured logging",
                "code": """
// LoggingMiddleware creates middleware that logs HTTP requests.
//
// Logs: method, path, status code, duration, request ID.
// Uses structured logging with zap or logrus.
func LoggingMiddleware(logger *zap.Logger) func(http.Handler) http.Handler {
    return func(next http.Handler) http.Handler {
        return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
            start := time.Now()
            requestID := uuid.New().String()

            // Add request ID to context
            ctx := context.WithValue(r.Context(), "request_id", requestID)
            r = r.WithContext(ctx)

            // Wrap response writer to capture status code
            wrapped := &responseWriter{ResponseWriter: w, statusCode: 200}

            // Process request
            next.ServeHTTP(wrapped, r)

            // Log request details
            duration := time.Since(start)
            logger.Info("HTTP Request",
                zap.String("method", r.Method),
                zap.String("path", r.URL.Path),
                zap.Int("status", wrapped.statusCode),
                zap.Duration("duration", duration),
                zap.String("request_id", requestID),
            )
        })
    }
}
""",
            },
        ],
    },
    "java": {
        "extension": ".java",
        "samples": [
            {
                "id": "java_circuit_breaker",
                "name": "CircuitBreaker",
                "type": "class",
                "description": "Circuit breaker pattern for resilient service calls",
                "code": """
/**
 * Circuit breaker implementation for protecting service calls.
 *
 * States: CLOSED (normal), OPEN (failing), HALF_OPEN (testing).
 * Prevents cascading failures in distributed systems.
 *
 * @param <T> Return type of the protected operation
 */
public class CircuitBreaker<T> {
    private final int failureThreshold;
    private final Duration timeout;
    private final AtomicInteger failureCount;
    private volatile State state;
    private volatile Instant lastFailure;

    public CircuitBreaker(int failureThreshold, Duration timeout) {
        this.failureThreshold = failureThreshold;
        this.timeout = timeout;
        this.failureCount = new AtomicInteger(0);
        this.state = State.CLOSED;
    }

    /**
     * Execute operation with circuit breaker protection.
     */
    public T execute(Supplier<T> operation) throws CircuitBreakerException {
        if (state == State.OPEN) {
            if (shouldTryReset()) {
                state = State.HALF_OPEN;
            } else {
                throw new CircuitBreakerException("Circuit is open");
            }
        }

        try {
            T result = operation.get();
            onSuccess();
            return result;
        } catch (Exception e) {
            onFailure();
            throw new CircuitBreakerException("Operation failed", e);
        }
    }

    private void onSuccess() {
        failureCount.set(0);
        state = State.CLOSED;
    }

    private void onFailure() {
        lastFailure = Instant.now();
        if (failureCount.incrementAndGet() >= failureThreshold) {
            state = State.OPEN;
        }
    }
}
""",
            },
        ],
    },
    "cpp": {
        "extension": ".cpp",
        "samples": [
            {
                "id": "cpp_thread_pool",
                "name": "ThreadPool",
                "type": "class",
                "description": "Thread pool with work stealing for efficient task scheduling",
                "code": """
/**
 * @brief High-performance thread pool with work stealing.
 *
 * Uses lock-free queues for minimal contention.
 * Supports task priorities and future-based results.
 *
 * @tparam T Result type for tasks
 */
template<typename T>
class ThreadPool {
public:
    explicit ThreadPool(size_t numThreads = std::thread::hardware_concurrency())
        : stop_(false), activeWorkers_(0) {
        workers_.reserve(numThreads);
        for (size_t i = 0; i < numThreads; ++i) {
            workers_.emplace_back([this, i] { workerLoop(i); });
        }
    }

    ~ThreadPool() {
        {
            std::lock_guard<std::mutex> lock(mutex_);
            stop_ = true;
        }
        condition_.notify_all();
        for (auto& worker : workers_) {
            worker.join();
        }
    }

    /**
     * @brief Submit a task to the pool.
     * @param task Callable task to execute
     * @return Future for the task result
     */
    template<typename F>
    std::future<T> submit(F&& task) {
        auto promise = std::make_shared<std::promise<T>>();
        auto future = promise->get_future();

        {
            std::lock_guard<std::mutex> lock(mutex_);
            tasks_.push([promise, task = std::forward<F>(task)]() mutable {
                try {
                    promise->set_value(task());
                } catch (...) {
                    promise->set_exception(std::current_exception());
                }
            });
        }
        condition_.notify_one();
        return future;
    }

private:
    void workerLoop(size_t id);

    std::vector<std::thread> workers_;
    std::queue<std::function<void()>> tasks_;
    std::mutex mutex_;
    std::condition_variable condition_;
    std::atomic<bool> stop_;
    std::atomic<size_t> activeWorkers_;
};
""",
            },
        ],
    },
    "ruby": {
        "extension": ".rb",
        "samples": [
            {
                "id": "rb_active_record",
                "name": "User",
                "type": "class",
                "description": "ActiveRecord model with validations and callbacks",
                "code": """
# User model with full validation, associations, and callbacks.
#
# Implements soft deletes, password hashing, and audit logging.
class User < ApplicationRecord
  # Associations
  has_many :posts, dependent: :destroy
  has_many :comments
  has_one :profile, dependent: :destroy
  belongs_to :organization, optional: true

  # Validations
  validates :email, presence: true,
                    uniqueness: { case_sensitive: false },
                    format: { with: URI::MailTo::EMAIL_REGEXP }
  validates :password, length: { minimum: 8 }, if: :password_required?
  validates :name, presence: true, length: { maximum: 100 }

  # Callbacks
  before_save :normalize_email
  before_create :generate_auth_token
  after_create :send_welcome_email
  before_destroy :cancel_subscriptions

  # Scopes
  scope :active, -> { where(deleted_at: nil) }
  scope :admins, -> { where(role: 'admin') }
  scope :created_this_month, -> { where(created_at: Time.current.beginning_of_month..) }

  def authenticate(password)
    BCrypt::Password.new(password_digest).is_password?(password)
  end

  def soft_delete
    update(deleted_at: Time.current)
  end

  private

  def normalize_email
    self.email = email.downcase.strip
  end

  def generate_auth_token
    self.auth_token = SecureRandom.urlsafe_base64(32)
  end
end
""",
            },
        ],
    },
    "sql": {
        "extension": ".sql",
        "samples": [
            {
                "id": "sql_analytics",
                "name": "monthly_revenue_report",
                "type": "query",
                "description": "Complex analytics query for monthly revenue with window functions",
                "code": """
-- Monthly revenue analytics with year-over-year comparison
-- Uses window functions for running totals and growth calculations
WITH monthly_revenue AS (
    SELECT
        DATE_TRUNC('month', o.created_at) AS month,
        p.category,
        SUM(oi.quantity * oi.unit_price) AS revenue,
        COUNT(DISTINCT o.customer_id) AS unique_customers,
        COUNT(DISTINCT o.id) AS order_count
    FROM orders o
    JOIN order_items oi ON o.id = oi.order_id
    JOIN products p ON oi.product_id = p.id
    WHERE o.status = 'completed'
      AND o.created_at >= DATE_TRUNC('year', CURRENT_DATE) - INTERVAL '1 year'
    GROUP BY DATE_TRUNC('month', o.created_at), p.category
),
with_growth AS (
    SELECT
        *,
        LAG(revenue) OVER (PARTITION BY category ORDER BY month) AS prev_month_revenue,
        SUM(revenue) OVER (PARTITION BY category ORDER BY month) AS running_total,
        RANK() OVER (PARTITION BY month ORDER BY revenue DESC) AS category_rank
    FROM monthly_revenue
)
SELECT
    month,
    category,
    revenue,
    unique_customers,
    order_count,
    ROUND((revenue - prev_month_revenue) / NULLIF(prev_month_revenue, 0) * 100, 2) AS mom_growth_pct,
    running_total,
    category_rank
FROM with_growth
ORDER BY month DESC, revenue DESC;
""",
            },
        ],
    },
    "bash": {
        "extension": ".sh",
        "samples": [
            {
                "id": "bash_deploy",
                "name": "deploy.sh",
                "type": "script",
                "description": "Deployment script with health checks and rollback",
                "code": """
#!/usr/bin/env bash
# Deploy script with zero-downtime deployment and automatic rollback
#
# Features:
# - Blue-green deployment
# - Health check verification
# - Automatic rollback on failure
# - Slack notifications

set -euo pipefail

DEPLOY_ENV="${1:-production}"
VERSION="${2:-latest}"
HEALTH_CHECK_TIMEOUT=60
ROLLBACK_ON_FAILURE=true

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1"
}

health_check() {
    local url=$1
    local timeout=$2
    local start=$(date +%s)

    while true; do
        if curl -sf "$url/health" > /dev/null 2>&1; then
            log "Health check passed"
            return 0
        fi

        local elapsed=$(($(date +%s) - start))
        if [ $elapsed -gt $timeout ]; then
            log "Health check failed after ${timeout}s"
            return 1
        fi

        sleep 2
    done
}

deploy() {
    log "Starting deployment of version $VERSION to $DEPLOY_ENV"

    # Pull new image
    docker pull "myapp:$VERSION"

    # Start new container
    docker run -d --name "myapp-$VERSION" -p 8081:8080 "myapp:$VERSION"

    # Health check new container
    if health_check "http://localhost:8081" $HEALTH_CHECK_TIMEOUT; then
        # Switch traffic
        docker stop myapp-current || true
        docker rename myapp-current myapp-old || true
        docker rename "myapp-$VERSION" myapp-current

        # Update load balancer
        update_load_balancer

        log "Deployment successful"
    else
        log "Deployment failed, rolling back"
        rollback
        exit 1
    fi
}

deploy
""",
            },
        ],
    },
    "kotlin": {
        "extension": ".kt",
        "samples": [
            {
                "id": "kt_coroutine",
                "name": "DataFetcher",
                "type": "class",
                "description": "Coroutine-based data fetcher with retry and caching",
                "code": """
/**
 * Data fetcher using Kotlin coroutines for async operations.
 *
 * Features:
 * - Structured concurrency
 * - Retry with exponential backoff
 * - In-memory caching with TTL
 * - Cancellation support
 */
class DataFetcher(
    private val httpClient: HttpClient,
    private val cache: Cache<String, Any>,
    private val dispatcher: CoroutineDispatcher = Dispatchers.IO
) {
    /**
     * Fetch data with automatic retry and caching.
     */
    suspend fun <T> fetch(
        url: String,
        parser: (String) -> T,
        maxRetries: Int = 3,
        cacheTtl: Duration = 5.minutes
    ): T = withContext(dispatcher) {
        // Check cache first
        cache.get(url)?.let {
            @Suppress("UNCHECKED_CAST")
            return@withContext it as T
        }

        // Fetch with retry
        val result = retry(maxRetries) {
            val response = httpClient.get(url)
            if (!response.status.isSuccess()) {
                throw HttpException(response.status)
            }
            parser(response.bodyAsText())
        }

        // Cache result
        cache.put(url, result, cacheTtl)
        result
    }

    private suspend fun <T> retry(
        maxAttempts: Int,
        block: suspend () -> T
    ): T {
        var lastException: Exception? = null
        repeat(maxAttempts) { attempt ->
            try {
                return block()
            } catch (e: Exception) {
                lastException = e
                delay((2.0.pow(attempt) * 1000).toLong())
            }
        }
        throw lastException ?: IllegalStateException("Retry failed")
    }
}
""",
            },
        ],
    },
    "swift": {
        "extension": ".swift",
        "samples": [
            {
                "id": "swift_network",
                "name": "NetworkManager",
                "type": "class",
                "description": "Async/await network manager with Combine support",
                "code": """
/// Network manager using modern Swift concurrency.
///
/// Features:
/// - async/await API
/// - Combine publisher support
/// - Request/response interceptors
/// - Automatic retry with backoff
actor NetworkManager {
    private let session: URLSession
    private let decoder: JSONDecoder
    private var interceptors: [RequestInterceptor]

    init(configuration: URLSessionConfiguration = .default) {
        self.session = URLSession(configuration: configuration)
        self.decoder = JSONDecoder()
        self.decoder.keyDecodingStrategy = .convertFromSnakeCase
        self.interceptors = []
    }

    /// Perform a network request with automatic decoding.
    func request<T: Decodable>(
        _ endpoint: Endpoint,
        type: T.Type
    ) async throws -> T {
        var request = endpoint.urlRequest

        // Apply interceptors
        for interceptor in interceptors {
            request = await interceptor.intercept(request)
        }

        let (data, response) = try await session.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw NetworkError.invalidResponse
        }

        guard 200...299 ~= httpResponse.statusCode else {
            throw NetworkError.httpError(httpResponse.statusCode)
        }

        return try decoder.decode(T.self, from: data)
    }

    /// Add a request interceptor.
    func addInterceptor(_ interceptor: RequestInterceptor) {
        interceptors.append(interceptor)
    }
}
""",
            },
        ],
    },
}


# =============================================================================
# Semantic Search Test Queries
# =============================================================================

SEMANTIC_SEARCH_QUERIES = [
    {
        "query": "async HTTP request with retry and timeout",
        "expected_matches": ["py_async_http", "kt_coroutine", "swift_network"],
        "category": "async_operations",
    },
    {
        "query": "caching function results with expiration",
        "expected_matches": ["py_cache_decorator", "kt_coroutine"],
        "category": "caching",
    },
    {
        "query": "database connection pooling",
        "expected_matches": ["py_db_repository", "rs_async_pool"],
        "category": "database",
    },
    {
        "query": "concurrent task queue with worker threads",
        "expected_matches": ["js_promise_queue", "go_worker_pool", "cpp_thread_pool"],
        "category": "concurrency",
    },
    {
        "query": "event handling and callbacks",
        "expected_matches": ["js_event_emitter"],
        "category": "events",
    },
    {
        "query": "error handling with context and retry",
        "expected_matches": ["rs_result_ext", "java_circuit_breaker"],
        "category": "error_handling",
    },
    {
        "query": "HTTP middleware for logging requests",
        "expected_matches": ["go_middleware"],
        "category": "middleware",
    },
    {
        "query": "graceful shutdown with cleanup",
        "expected_matches": ["go_worker_pool", "cpp_thread_pool", "bash_deploy"],
        "category": "lifecycle",
    },
    {
        "query": "data validation and sanitization",
        "expected_matches": ["rb_active_record"],
        "category": "validation",
    },
    {
        "query": "analytics query with aggregation",
        "expected_matches": ["sql_analytics"],
        "category": "analytics",
    },
]


# =============================================================================
# Helper Functions
# =============================================================================


def generate_embedding(text: str, dimension: int = 384) -> List[float]:
    """Generate deterministic pseudo-embedding from text."""
    text_hash = hashlib.sha256(text.encode()).digest()
    embedding = []
    for i in range(dimension):
        byte_idx = i % len(text_hash)
        next_byte = text_hash[(i + 1) % len(text_hash)]
        value = (text_hash[byte_idx] + next_byte * 0.1 - 128) / 128.0
        embedding.append(value)
    magnitude = sum(v * v for v in embedding) ** 0.5
    if magnitude > 0:
        embedding = [v / magnitude for v in embedding]
    return embedding


def to_sql_value(value: Any) -> Dict[str, Any]:
    """Convert Python value to SqlValue format."""
    if value is None:
        return {"null_value": 0}
    elif isinstance(value, bool):
        return {"bool_value": value}
    elif isinstance(value, int):
        return {"int64_value": value}
    elif isinstance(value, float):
        return {"number_value": value}
    elif isinstance(value, str):
        return {"string_value": value}
    elif isinstance(value, (list, tuple)):
        return {"array_value": {"values": [to_sql_value(v) for v in value]}}
    else:
        return {"string_value": str(value)}


def convert_metadata(metadata: Dict[str, Any]) -> Dict[str, Any]:
    """Convert metadata dict to SqlValue format."""
    return {k: to_sql_value(v) for k, v in metadata.items()}


def create_collection(name: str, dimension: int = 384) -> Dict[str, Any]:
    """Create a collection."""
    response = requests.post(
        f"{SERVER_URL}/api/v1/collections",
        json={
            "operation": 1,
            "collection_id": name,
            "collection_config": {
                "name": name,
                "dimension": dimension,
                "distance_metric": 1,
            },
        },
    )
    return response.json()


def delete_collection(name: str) -> bool:
    """Delete a collection."""
    try:
        response = requests.delete(f"{SERVER_URL}/api/v1/collections/{name}")
        return response.status_code in (200, 204, 404)
    except Exception:
        return False


def insert_vectors(
    collection_name: str, vectors: List[Dict[str, Any]]
) -> Dict[str, Any]:
    """Insert vectors with metadata."""
    formatted_vectors = []
    for v in vectors:
        formatted_v = {"id": v["id"], "vector": v["vector"]}
        if "metadata" in v:
            formatted_v["metadata"] = convert_metadata(v["metadata"])
        formatted_vectors.append(formatted_v)

    response = requests.post(
        f"{SERVER_URL}/api/v1/vectors/batch",
        json={"collection_id": collection_name, "vectors": formatted_vectors},
    )
    return response.json()


def search_vectors(
    collection_name: str, query_vector: List[float], top_k: int = 10
) -> List[Dict[str, Any]]:
    """Search for similar vectors. Returns list of results."""
    response = requests.post(
        f"{SERVER_URL}/api/v1/search",
        json={
            "collection_id": collection_name,
            "queries": [{"vector": query_vector}],
            "top_k": top_k,
        },
    )
    data = response.json()

    # Handle nested result structure: data["results"]["results"]
    if data.get("success") and data.get("results"):
        inner_results = data["results"]
        if isinstance(inner_results, dict) and "results" in inner_results:
            return inner_results["results"] or []
        elif isinstance(inner_results, list):
            return inner_results
    return []


def assert_success(result: Dict[str, Any], message: str = "API call failed"):
    """Assert API call succeeded."""
    if result.get("success") is True:
        return
    if result.get("error_message") not in (None, ""):
        raise AssertionError(f"{message}: {result}")
    if result.get("error"):
        raise AssertionError(f"{message}: {result}")


# =============================================================================
# Test Classes
# =============================================================================


@dataclass
class SearchResult:
    """Semantic search test result."""

    query: str
    expected_ids: List[str]
    found_ids: List[str]
    top_k_accuracy: float
    mrr: float  # Mean Reciprocal Rank
    recall: float


class TestMultiLanguageIndexing:
    """Test indexing code from multiple languages."""

    @pytest.fixture(scope="class")
    def multilang_collection(self):
        """Create and populate a collection with all language samples."""
        collection_name = f"test_multilang_{int(time.time() * 1000)}"

        # Create collection
        result = create_collection(collection_name)
        assert_success(result, "Failed to create collection")

        # Index all samples
        vectors = []
        for lang, lang_data in LANGUAGE_SAMPLES.items():
            for sample in lang_data["samples"]:
                # Create embedding from description + code
                embedding_text = (
                    f"{sample['name']} {sample['description']} {sample['code'][:500]}"
                )
                vectors.append(
                    {
                        "id": sample["id"],
                        "vector": generate_embedding(embedding_text),
                        "metadata": {
                            "language": lang,
                            "name": sample["name"],
                            "type": sample["type"],
                            "description": sample["description"],
                        },
                    }
                )

        result = insert_vectors(collection_name, vectors)
        assert_success(result, "Failed to insert vectors")

        yield collection_name

        # Cleanup
        delete_collection(collection_name)

    def test_all_languages_indexed(self, multilang_collection):
        """Verify all language samples were indexed."""
        total_samples = sum(len(data["samples"]) for data in LANGUAGE_SAMPLES.values())

        # Count by searching with a generic query
        query = generate_embedding("function class method")
        results = search_vectors(multilang_collection, query, top_k=total_samples + 10)

        # Should have results
        assert len(results) > 0, "No search results returned"
        print(f"\nTotal samples indexed: {total_samples}")
        print(f"Results returned: {len(results)}")
        print(f"Languages: {list(LANGUAGE_SAMPLES.keys())}")

    def test_language_specific_search(self, multilang_collection):
        """Test searching for language-specific patterns."""
        test_cases = [
            ("async await coroutine", ["python", "kotlin", "swift"]),
            ("goroutine channel", ["go"]),
            ("trait impl", ["rust"]),
            ("activerecord model", ["ruby"]),
        ]

        for query_text, expected_langs in test_cases:
            query = generate_embedding(query_text)
            results = search_vectors(multilang_collection, query, top_k=5)

            print(f"\nQuery: '{query_text}'")
            print(f"  Expected langs: {expected_langs}")
            print(f"  Found {len(results)} results")
            for r in results[:3]:
                print(
                    f"    - {r.get('id', 'unknown')} (score: {r.get('score', 0):.4f})"
                )


class TestSemanticSearchEffectiveness:
    """Evaluate semantic search quality against real-world queries."""

    @pytest.fixture(scope="class")
    def search_collection(self):
        """Create collection for semantic search tests."""
        collection_name = f"test_semantic_{int(time.time() * 1000)}"

        result = create_collection(collection_name)
        assert_success(result, "Failed to create collection")

        # Index all samples
        vectors = []
        for lang, lang_data in LANGUAGE_SAMPLES.items():
            for sample in lang_data["samples"]:
                embedding_text = (
                    f"{sample['name']} {sample['description']} {sample['code'][:1000]}"
                )
                vectors.append(
                    {
                        "id": sample["id"],
                        "vector": generate_embedding(embedding_text),
                        "metadata": {
                            "language": lang,
                            "name": sample["name"],
                            "type": sample["type"],
                            "description": sample["description"],
                        },
                    }
                )

        result = insert_vectors(collection_name, vectors)
        assert_success(result, "Failed to insert vectors")

        yield collection_name

        delete_collection(collection_name)

    def test_semantic_search_queries(self, search_collection):
        """Test semantic search with real-world queries."""
        results = []

        for query_info in SEMANTIC_SEARCH_QUERIES:
            query_text = query_info["query"]
            expected_ids = set(query_info["expected_matches"])

            query_vector = generate_embedding(query_text)
            search_results = search_vectors(search_collection, query_vector, top_k=5)

            found_ids = []
            for r in search_results:
                found_ids.append(r.get("id", ""))

            # Calculate metrics
            found_set = set(found_ids)
            hits = expected_ids & found_set

            # Top-K accuracy: % of expected in top-K
            top_k_accuracy = len(hits) / len(expected_ids) if expected_ids else 0

            # Recall: % of expected found
            recall = len(hits) / len(expected_ids) if expected_ids else 0

            # MRR: 1/rank of first expected hit
            mrr = 0.0
            for i, fid in enumerate(found_ids):
                if fid in expected_ids:
                    mrr = 1.0 / (i + 1)
                    break

            search_result = SearchResult(
                query=query_text,
                expected_ids=list(expected_ids),
                found_ids=found_ids,
                top_k_accuracy=top_k_accuracy,
                mrr=mrr,
                recall=recall,
            )
            results.append(search_result)

            print(f"\n{'='*60}")
            print(f"Query: {query_text}")
            print(f"Category: {query_info['category']}")
            print(f"Expected: {expected_ids}")
            print(f"Found: {found_ids[:5]}")
            print(f"Hits: {hits}")
            print(f"Top-K Accuracy: {top_k_accuracy:.2%}")
            print(f"Recall: {recall:.2%}")
            print(f"MRR: {mrr:.3f}")

        # Aggregate metrics
        avg_accuracy = sum(r.top_k_accuracy for r in results) / len(results)
        avg_recall = sum(r.recall for r in results) / len(results)
        avg_mrr = sum(r.mrr for r in results) / len(results)

        print(f"\n{'='*60}")
        print("AGGREGATE SEMANTIC SEARCH METRICS")
        print(f"{'='*60}")
        print(f"Average Top-K Accuracy: {avg_accuracy:.2%}")
        print(f"Average Recall: {avg_recall:.2%}")
        print(f"Average MRR: {avg_mrr:.3f}")
        print(f"Total Queries: {len(results)}")

        # Store for reporting
        self._search_results = results


class TestCrossLanguagePatterns:
    """Test finding similar patterns across different languages."""

    @pytest.fixture(scope="class")
    def pattern_collection(self):
        """Create collection for cross-language pattern tests."""
        collection_name = f"test_patterns_{int(time.time() * 1000)}"

        result = create_collection(collection_name)
        assert_success(result, "Failed to create collection")

        vectors = []
        for lang, lang_data in LANGUAGE_SAMPLES.items():
            for sample in lang_data["samples"]:
                embedding_text = (
                    f"{sample['name']} {sample['description']} {sample['code'][:1000]}"
                )
                vectors.append(
                    {
                        "id": sample["id"],
                        "vector": generate_embedding(embedding_text),
                        "metadata": {
                            "language": lang,
                            "name": sample["name"],
                            "type": sample["type"],
                            "description": sample["description"],
                        },
                    }
                )

        result = insert_vectors(collection_name, vectors)
        assert_success(result, "Failed to insert vectors")

        yield collection_name
        delete_collection(collection_name)

    def test_find_async_patterns(self, pattern_collection):
        """Find async/await patterns across languages."""
        query = generate_embedding(
            "asynchronous programming async await promise future"
        )
        results = search_vectors(pattern_collection, query, top_k=10)

        print("\n=== ASYNC PATTERNS ACROSS LANGUAGES ===")
        for r in results:
            print(f"  {r.get('id', 'unknown')} (score: {r.get('score', 0):.4f})")

    def test_find_error_handling_patterns(self, pattern_collection):
        """Find error handling patterns across languages."""
        query = generate_embedding("error handling exception try catch result")
        results = search_vectors(pattern_collection, query, top_k=10)

        print("\n=== ERROR HANDLING PATTERNS ACROSS LANGUAGES ===")
        for r in results:
            print(f"  {r.get('id', 'unknown')} (score: {r.get('score', 0):.4f})")

    def test_find_concurrency_patterns(self, pattern_collection):
        """Find concurrency patterns across languages."""
        query = generate_embedding("thread pool worker queue concurrent parallel")
        results = search_vectors(pattern_collection, query, top_k=10)

        print("\n=== CONCURRENCY PATTERNS ACROSS LANGUAGES ===")
        for r in results:
            print(f"  {r.get('id', 'unknown')} (score: {r.get('score', 0):.4f})")


class TestRealWorldCodeSearch:
    """Test searching for real-world coding tasks."""

    @pytest.fixture(scope="class")
    def code_collection(self):
        """Create collection with all code samples."""
        collection_name = f"test_realworld_{int(time.time() * 1000)}"

        result = create_collection(collection_name)
        assert_success(result, "Failed to create collection")

        vectors = []
        for lang, lang_data in LANGUAGE_SAMPLES.items():
            for sample in lang_data["samples"]:
                embedding_text = (
                    f"{sample['name']} {sample['description']} {sample['code'][:1000]}"
                )
                vectors.append(
                    {
                        "id": sample["id"],
                        "vector": generate_embedding(embedding_text),
                        "metadata": {
                            "language": lang,
                            "name": sample["name"],
                            "type": sample["type"],
                            "description": sample["description"],
                        },
                    }
                )

        result = insert_vectors(collection_name, vectors)
        assert_success(result, "Failed to insert vectors")

        yield collection_name
        delete_collection(collection_name)

    def test_developer_queries(self, code_collection):
        """Test real developer search queries."""
        developer_queries = [
            "How do I implement connection pooling?",
            "Show me caching with TTL",
            "I need to limit concurrent operations",
            "How to add logging to HTTP requests",
            "Implement retry with exponential backoff",
            "Database CRUD operations pattern",
            "How to do deployment with rollback",
            "Event pub-sub implementation",
        ]

        print("\n" + "=" * 70)
        print("REAL-WORLD DEVELOPER QUERY RESULTS")
        print("=" * 70)

        for query_text in developer_queries:
            query = generate_embedding(query_text)
            results = search_vectors(code_collection, query, top_k=3)

            print(f'\nQuery: "{query_text}"')
            print("-" * 50)

            for i, r in enumerate(results[:3], 1):
                score = r.get("score", 0)
                rid = r.get("id", "?")
                print(f"  {i}. {rid} (score: {score:.4f})")


class TestSearchPerformance:
    """Benchmark search performance."""

    @pytest.fixture(scope="class")
    def perf_collection(self):
        """Create collection for performance tests."""
        collection_name = f"test_perf_{int(time.time() * 1000)}"

        result = create_collection(collection_name)
        assert_success(result, "Failed to create collection")

        # Insert all samples
        vectors = []
        for lang, lang_data in LANGUAGE_SAMPLES.items():
            for sample in lang_data["samples"]:
                embedding_text = (
                    f"{sample['name']} {sample['description']} {sample['code'][:1000]}"
                )
                vectors.append(
                    {
                        "id": sample["id"],
                        "vector": generate_embedding(embedding_text),
                        "metadata": {
                            "language": lang,
                            "name": sample["name"],
                            "type": sample["type"],
                        },
                    }
                )

        result = insert_vectors(collection_name, vectors)
        assert_success(result, "Failed to insert vectors")

        yield collection_name
        delete_collection(collection_name)

    def test_search_latency(self, perf_collection):
        """Measure search latency."""
        query = generate_embedding("async function with error handling")

        latencies = []
        for _ in range(20):
            start = time.time()
            search_vectors(perf_collection, query, top_k=10)
            latencies.append((time.time() - start) * 1000)

        avg_latency = sum(latencies) / len(latencies)
        p50 = sorted(latencies)[len(latencies) // 2]
        p95 = sorted(latencies)[int(len(latencies) * 0.95)]
        p99 = sorted(latencies)[int(len(latencies) * 0.99)]

        print(f"\n{'='*50}")
        print("SEARCH LATENCY (ms)")
        print(f"{'='*50}")
        print(f"Average: {avg_latency:.2f}ms")
        print(f"P50: {p50:.2f}ms")
        print(f"P95: {p95:.2f}ms")
        print(f"P99: {p99:.2f}ms")
        print(f"Min: {min(latencies):.2f}ms")
        print(f"Max: {max(latencies):.2f}ms")

        # Performance assertion
        assert avg_latency < 100, f"Average latency too high: {avg_latency}ms"


if __name__ == "__main__":
    pytest.main([__file__, "-v", "-s"])
