/**
 * Sample Swift file for testing code chunking.
 *
 * This file contains various Swift constructs to test AST parsing.
 */

import Foundation

// MARK: - Constants

let maxRetries: Int = 3
let defaultTimeout: Double = 30.0

// MARK: - Custom Errors

enum ServiceError: Error {
    case invalidInput(String)
    case notFound(String)
    case internalError(String)
}

// MARK: - Protocols

protocol Service {
    func initialize() async throws
    var isReady: Bool { get }
}

protocol Displayable {
    var displayName: String { get }
}

// MARK: - User

/**
 * Represents a user in the system.
 */
struct User: Displayable, Codable, Equatable {
    let id: String
    let name: String
    var email: String?

    var displayName: String {
        if !name.isEmpty {
            return name
        }
        return email ?? id
    }

    init(id: String, name: String, email: String? = nil) {
        self.id = id
        self.name = name
        self.email = email
    }
}

// MARK: - Service Status

/**
 * Enum for service status.
 */
enum ServiceStatus {
    case pending
    case running
    case stopped
    case error(Error)
}

// MARK: - Base Service

/**
 * Base class for services.
 */
class BaseService: Service {
    let config: [String: Any]
    private(set) var initialized = false

    var isReady: Bool { initialized }

    init(config: [String: Any] = [:]) {
        self.config = config
    }

    func initialize() async throws {
        initialized = true
    }

    func validateConfig() -> Bool {
        return !config.isEmpty
    }
}

// MARK: - User Service

/**
 * Service for managing users.
 */
class UserService: BaseService {
    private var users: [String: User] = [:]

    func createUser(id: String, name: String, email: String? = nil) throws -> User {
        guard !id.isEmpty else {
            throw ServiceError.invalidInput("ID cannot be empty")
        }

        let user = User(id: id, name: name, email: email)
        users[id] = user
        onUserCreated(user)
        return user
    }

    func getUser(id: String) -> User? {
        return users[id]
    }

    func deleteUser(id: String) -> Bool {
        return users.removeValue(forKey: id) != nil
    }

    func getAllUsers() -> [User] {
        return Array(users.values)
    }

    private func onUserCreated(_ user: User) {
        // Internal callback
    }
}

// MARK: - Math Utilities

/**
 * Utility struct for math operations.
 */
struct MathUtils {
    /**
     * Calculate factorial of n.
     */
    static func calculateFactorial(_ n: UInt64) -> UInt64 {
        if n <= 1 { return 1 }
        return n * calculateFactorial(n - 1)
    }

    /**
     * Calculate fibonacci number.
     */
    static func fibonacci(_ n: Int) -> Int {
        guard n > 1 else { return n }
        return fibonacci(n - 1) + fibonacci(n - 2)
    }
}

// MARK: - Data Fetching

/**
 * Fetch data from URL asynchronously.
 */
func fetchData(url: String, timeout: Double = defaultTimeout) async throws -> [String: String] {
    // Simulated async fetch
    return ["url": url, "status": "ok"]
}

/**
 * Process items with optional validation.
 */
func processItems(_ items: [String], validate: Bool = true) -> [String] {
    var filtered = validate ? items.filter { !$0.isEmpty } : items
    return filtered.map { $0.trimmingCharacters(in: .whitespaces).lowercased() }
}

// MARK: - Generic Container

/**
 * Generic container class.
 */
class Container<T> {
    private var value: T

    init(_ value: T) {
        self.value = value
    }

    func get() -> T { value }
    func set(_ value: T) { self.value = value }
}

// MARK: - Extensions

extension String {
    func truncate(_ maxLength: Int) -> String {
        if count <= maxLength { return self }
        return String(prefix(maxLength))
    }
}

// MARK: - Typealiases

typealias UserId = String
typealias UserMap = [UserId: User]

// MARK: - Higher-order Functions

func withRetry<T>(maxRetries: Int = maxRetries, block: () throws -> T) rethrows -> T {
    var lastError: Error?
    for _ in 0..<maxRetries {
        do {
            return try block()
        } catch {
            lastError = error
        }
    }
    throw lastError!
}

// MARK: - Main

@main
struct Main {
    static func main() async {
        let service = UserService(config: ["env": "test"])
        do {
            try await service.initialize()

            let user = try service.createUser(id: "1", name: "Test User", email: "test@example.com")
            print("Created user: \(user.displayName)")

            let result = MathUtils.calculateFactorial(5)
            print("Factorial: \(result)")
        } catch {
            print("Error: \(error)")
        }
    }
}
