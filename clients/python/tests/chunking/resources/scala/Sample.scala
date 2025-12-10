/**
 * Sample Scala file for testing code chunking.
 *
 * This file contains various Scala constructs to test AST parsing.
 */
package sample

import scala.concurrent.{Future, ExecutionContext}
import scala.util.{Try, Success, Failure}

// Constants
object Constants {
  val MaxRetries: Int = 3
  val DefaultTimeout: Double = 30.0
}

/**
 * Represents a user in the system.
 */
case class User(
    id: String,
    name: String,
    email: Option[String] = None
) {
  def getDisplayName: String = {
    if (name.nonEmpty) name
    else email.getOrElse(id)
  }

  def withEmail(newEmail: String): User = copy(email = Some(newEmail))
}

/**
 * Custom exception for service errors.
 */
case class ServiceException(message: String, errorCode: Option[String] = None)
    extends Exception(message)

/**
 * Trait for services.
 */
trait Service {
  def initialize(): Unit
  def isReady: Boolean
}

/**
 * Sealed trait for results.
 */
sealed trait Result[+A]
case class Success[A](value: A) extends Result[A]
case class Error(message: String, code: Option[String] = None) extends Result[Nothing]

/**
 * Enumeration for service status.
 */
object ServiceStatus extends Enumeration {
  type ServiceStatus = Value
  val Pending, Running, Stopped, Error = Value
}

/**
 * Abstract class for services.
 */
abstract class BaseService(config: Map[String, Any]) extends Service {
  protected var initialized: Boolean = false

  override def initialize(): Unit = {
    initialized = true
  }

  override def isReady: Boolean = initialized

  protected def validateConfig: Boolean = config.nonEmpty
}

/**
 * Service for managing users.
 */
class UserService(config: Map[String, Any] = Map.empty) extends BaseService(config) {
  private val users = scala.collection.mutable.Map[String, User]()

  def createUser(id: String, name: String, email: Option[String] = None): User = {
    if (id.isEmpty) {
      throw ServiceException("ID cannot be empty", Some("INVALID_ID"))
    }
    val user = User(id, name, email)
    users(id) = user
    onUserCreated(user)
    user
  }

  def getUser(id: String): Option[User] = users.get(id)

  def deleteUser(id: String): Boolean = users.remove(id).isDefined

  def getAllUsers: List[User] = users.values.toList

  private def onUserCreated(user: User): Unit = {
    // Internal callback
  }
}

/**
 * Object with utility methods.
 */
object MathUtils {
  /**
   * Calculate factorial of n.
   */
  def calculateFactorial(n: Long): Long = {
    if (n <= 1) 1L else n * calculateFactorial(n - 1)
  }

  /**
   * Calculate fibonacci number.
   */
  def fibonacci(n: Int): Long = n match {
    case 0 => 0L
    case 1 => 1L
    case _ => fibonacci(n - 1) + fibonacci(n - 2)
  }

  /**
   * Higher-order function for retry logic.
   */
  def withRetry[A](maxRetries: Int = Constants.MaxRetries)(block: => A): A = {
    var lastException: Throwable = null
    for (_ <- 1 to maxRetries) {
      try {
        return block
      } catch {
        case e: Throwable => lastException = e
      }
    }
    throw lastException
  }
}

/**
 * Fetch data asynchronously.
 */
object DataFetcher {
  def fetchData(url: String, timeout: Double = Constants.DefaultTimeout)(
      implicit ec: ExecutionContext
  ): Future[Map[String, String]] = {
    Future {
      Map("url" -> url, "status" -> "ok")
    }
  }
}

/**
 * Process items with optional validation.
 */
def processItems(items: List[String], validate: Boolean = true): List[String] = {
  val filtered = if (validate) items.filter(_.nonEmpty) else items
  filtered.map(_.trim.toLowerCase)
}

/**
 * Type alias.
 */
type UserId = String
type UserMap = Map[UserId, User]

/**
 * Implicit class for extensions.
 */
implicit class StringOps(val s: String) extends AnyVal {
  def truncate(maxLength: Int): String = {
    if (s.length <= maxLength) s else s.substring(0, maxLength)
  }
}

/**
 * Companion object with factory method.
 */
object User {
  def apply(id: String, name: String, email: String): User = {
    new User(id, name, Some(email))
  }
}

/**
 * Main entry point.
 */
object Main extends App {
  val service = new UserService(Map("env" -> "test"))
  service.initialize()

  val user = service.createUser("1", "Test User", Some("test@example.com"))
  println(s"Created user: ${user.getDisplayName}")

  val result = MathUtils.calculateFactorial(5)
  println(s"Factorial: $result")
}
