/**
 * Sample Kotlin file for testing code chunking.
 *
 * This file contains various Kotlin constructs to test AST parsing.
 */
package sample

import kotlin.math.sqrt

// Constants
const val MAX_RETRIES = 3
const val DEFAULT_TIMEOUT = 30.0

/**
 * Represents a user in the system.
 */
data class User(
    val id: String,
    val name: String,
    var email: String? = null
) {
    fun getDisplayName(): String = name.ifEmpty { email ?: id }
}

/**
 * Custom exception for service errors.
 */
class ServiceException(message: String, val errorCode: String? = null) : Exception(message)

/**
 * Interface for services.
 */
interface Service {
    suspend fun initialize()
    fun isReady(): Boolean
}

/**
 * Sealed class for service results.
 */
sealed class Result<out T> {
    data class Success<T>(val data: T) : Result<T>()
    data class Error(val message: String, val code: String? = null) : Result<Nothing>()
}

/**
 * Enum for service status.
 */
enum class ServiceStatus {
    PENDING,
    RUNNING,
    STOPPED,
    ERROR
}

/**
 * Base class for services.
 */
abstract class BaseService(protected val config: Map<String, Any>) : Service {
    protected var initialized = false

    override suspend fun initialize() {
        initialized = true
    }

    override fun isReady(): Boolean = initialized

    protected fun validateConfig(): Boolean = config.isNotEmpty()
}

/**
 * Service for managing users.
 */
class UserService(config: Map<String, Any> = emptyMap()) : BaseService(config) {
    private val users = mutableMapOf<String, User>()

    fun createUser(id: String, name: String, email: String? = null): User {
        if (id.isEmpty()) {
            throw ServiceException("ID cannot be empty", "INVALID_ID")
        }
        val user = User(id, name, email)
        users[id] = user
        onUserCreated(user)
        return user
    }

    fun getUser(id: String): User? = users[id]

    fun deleteUser(id: String): Boolean = users.remove(id) != null

    fun getAllUsers(): List<User> = users.values.toList()

    private fun onUserCreated(user: User) {
        // Internal callback
    }
}

/**
 * Calculate factorial of n.
 */
fun calculateFactorial(n: Long): Long {
    return if (n <= 1) 1 else n * calculateFactorial(n - 1)
}

/**
 * Fetch data from URL (simulated).
 */
suspend fun fetchData(url: String, timeout: Double = DEFAULT_TIMEOUT): Map<String, String> {
    // Simulated fetch
    return mapOf("url" to url, "status" to "ok")
}

/**
 * Process items with optional validation.
 */
fun processItems(items: List<String>, validate: Boolean = true): List<String> {
    val filtered = if (validate) items.filter { it.isNotEmpty() } else items
    return filtered.map { it.trim().lowercase() }
}

/**
 * Extension function for String.
 */
fun String.truncate(maxLength: Int): String {
    return if (length <= maxLength) this else substring(0, maxLength)
}

/**
 * Object for utility functions.
 */
object MathUtils {
    fun fibonacci(n: Int): Long {
        return if (n <= 1) n.toLong() else fibonacci(n - 1) + fibonacci(n - 2)
    }

    fun distance(x: Double, y: Double): Double = sqrt(x * x + y * y)
}

/**
 * Inline function with reified type.
 */
inline fun <reified T> createInstance(): T? {
    return try {
        T::class.java.getDeclaredConstructor().newInstance()
    } catch (e: Exception) {
        null
    }
}

/**
 * Higher-order function.
 */
fun <T> withRetry(maxRetries: Int = MAX_RETRIES, block: () -> T): T {
    var lastException: Exception? = null
    repeat(maxRetries) {
        try {
            return block()
        } catch (e: Exception) {
            lastException = e
        }
    }
    throw lastException ?: ServiceException("Unknown error")
}

/**
 * Main entry point.
 */
suspend fun main() {
    val service = UserService(mapOf("env" to "test"))
    service.initialize()

    val user = service.createUser("1", "Test User", "test@example.com")
    println("Created user: ${user.getDisplayName()}")

    val result = calculateFactorial(5)
    println("Factorial: $result")
}
