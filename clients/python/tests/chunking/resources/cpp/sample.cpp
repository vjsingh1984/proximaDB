/**
 * Sample C++ file for testing code chunking.
 *
 * This file contains various C++ constructs to test AST parsing.
 */

#include <iostream>
#include <string>
#include <unordered_map>
#include <optional>
#include <memory>
#include <vector>
#include <functional>
#include <future>

namespace sample {

// Constants
constexpr int MAX_RETRIES = 3;
constexpr double DEFAULT_TIMEOUT = 30.0;

/**
 * Represents a user in the system.
 */
class User {
public:
    User(std::string id, std::string name)
        : id_(std::move(id)), name_(std::move(name)) {}

    User(std::string id, std::string name, std::string email)
        : id_(std::move(id)), name_(std::move(name)), email_(std::move(email)) {}

    ~User() = default;

    // Copy constructor and assignment
    User(const User&) = default;
    User& operator=(const User&) = default;

    // Move constructor and assignment
    User(User&&) noexcept = default;
    User& operator=(User&&) noexcept = default;

    [[nodiscard]] const std::string& get_id() const { return id_; }
    [[nodiscard]] const std::string& get_name() const { return name_; }
    [[nodiscard]] const std::string& get_email() const { return email_; }

    void set_email(std::string email) { email_ = std::move(email); }

    [[nodiscard]] std::string get_display_name() const {
        if (!name_.empty()) return name_;
        if (!email_.empty()) return email_;
        return id_;
    }

private:
    std::string id_;
    std::string name_;
    std::string email_;
};

/**
 * Custom exception for service errors.
 */
class ServiceException : public std::exception {
public:
    explicit ServiceException(std::string message) : message_(std::move(message)) {}
    [[nodiscard]] const char* what() const noexcept override { return message_.c_str(); }

private:
    std::string message_;
};

/**
 * Interface for services.
 */
class IService {
public:
    virtual ~IService() = default;
    virtual void initialize() = 0;
    [[nodiscard]] virtual bool is_ready() const = 0;
};

/**
 * Service for managing users.
 */
class UserService : public IService {
public:
    UserService() : initialized_(false) {}
    ~UserService() override = default;

    void initialize() override {
        initialized_ = true;
    }

    [[nodiscard]] bool is_ready() const override {
        return initialized_;
    }

    std::shared_ptr<User> create_user(const std::string& id, const std::string& name) {
        if (id.empty()) {
            throw ServiceException("ID cannot be empty");
        }
        auto user = std::make_shared<User>(id, name);
        users_[id] = user;
        on_user_created(user);
        return user;
    }

    [[nodiscard]] std::optional<std::shared_ptr<User>> get_user(const std::string& id) const {
        auto it = users_.find(id);
        if (it != users_.end()) {
            return it->second;
        }
        return std::nullopt;
    }

    bool delete_user(const std::string& id) {
        return users_.erase(id) > 0;
    }

private:
    void on_user_created(const std::shared_ptr<User>& user) {
        // Internal callback
    }

    std::unordered_map<std::string, std::shared_ptr<User>> users_;
    bool initialized_;
};

/**
 * Template class for containers.
 */
template<typename T>
class Container {
public:
    explicit Container(T value) : value_(std::move(value)) {}

    [[nodiscard]] const T& get() const { return value_; }
    void set(T value) { value_ = std::move(value); }

private:
    T value_;
};

/**
 * Calculate factorial of n.
 */
constexpr unsigned long calculate_factorial(unsigned int n) {
    if (n <= 1) {
        return 1;
    }
    return n * calculate_factorial(n - 1);
}

/**
 * Fetch data asynchronously.
 */
std::future<std::unordered_map<std::string, std::string>> fetch_data(const std::string& url) {
    return std::async(std::launch::async, [url]() {
        std::unordered_map<std::string, std::string> result;
        result["url"] = url;
        result["status"] = "ok";
        return result;
    });
}

/**
 * Process items with optional validation.
 */
std::vector<std::string> process_items(const std::vector<std::string>& items, bool validate = true) {
    std::vector<std::string> result;
    for (const auto& item : items) {
        if (validate && item.empty()) {
            continue;
        }
        result.push_back(item);
    }
    return result;
}

// Lambda expression stored in variable
auto double_value = [](int x) { return x * 2; };

// Function pointer type
using Callback = std::function<void(const std::string&)>;

} // namespace sample

/**
 * Main entry point.
 */
int main() {
    using namespace sample;

    UserService service;
    service.initialize();

    try {
        auto user = service.create_user("1", "Test User");
        std::cout << "Created user: " << user->get_display_name() << std::endl;
    } catch (const ServiceException& e) {
        std::cerr << "Error: " << e.what() << std::endl;
        return 1;
    }

    auto result = calculate_factorial(5);
    std::cout << "Factorial: " << result << std::endl;

    return 0;
}
