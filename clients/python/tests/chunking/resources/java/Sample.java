/**
 * Sample Java file for testing code chunking.
 *
 * This file contains various Java constructs to test AST parsing.
 */
package sample;

import java.util.HashMap;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;

/**
 * Maximum number of retries for operations
 */
public final class Constants {
    public static final int MAX_RETRIES = 3;
    public static final double DEFAULT_TIMEOUT = 30.0;

    private Constants() {}
}

/**
 * Represents a user in the system.
 */
public class User {
    private String id;
    private String name;
    private String email;

    public User(String id, String name) {
        this.id = id;
        this.name = name;
    }

    public User(String id, String name, String email) {
        this.id = id;
        this.name = name;
        this.email = email;
    }

    public String getId() {
        return id;
    }

    public String getName() {
        return name;
    }

    public String getEmail() {
        return email;
    }

    public void setEmail(String email) {
        this.email = email;
    }

    public String getDisplayName() {
        return name != null ? name : email != null ? email : id;
    }
}

/**
 * Interface for services.
 */
public interface Service {
    void initialize() throws ServiceException;
    boolean isReady();
}

/**
 * Custom exception for service errors.
 */
public class ServiceException extends Exception {
    public ServiceException(String message) {
        super(message);
    }

    public ServiceException(String message, Throwable cause) {
        super(message, cause);
    }
}

/**
 * Service for managing users.
 */
public class UserService implements Service {
    private Map<String, User> users;
    private boolean initialized;

    public UserService() {
        this.users = new HashMap<>();
        this.initialized = false;
    }

    @Override
    public void initialize() throws ServiceException {
        this.initialized = true;
    }

    @Override
    public boolean isReady() {
        return initialized;
    }

    public User createUser(String id, String name) throws ServiceException {
        if (id == null || id.isEmpty()) {
            throw new ServiceException("ID cannot be empty");
        }
        User user = new User(id, name);
        users.put(id, user);
        onUserCreated(user);
        return user;
    }

    public Optional<User> getUser(String id) {
        return Optional.ofNullable(users.get(id));
    }

    public boolean deleteUser(String id) {
        return users.remove(id) != null;
    }

    private void onUserCreated(User user) {
        // Internal callback
    }
}

/**
 * Utility class with static methods.
 */
public class MathUtils {

    /**
     * Calculate factorial of n.
     */
    public static long calculateFactorial(int n) {
        if (n <= 1) {
            return 1;
        }
        return n * calculateFactorial(n - 1);
    }

    /**
     * Async method to fetch data.
     */
    public static CompletableFuture<Map<String, String>> fetchData(String url) {
        return CompletableFuture.supplyAsync(() -> {
            Map<String, String> result = new HashMap<>();
            result.put("url", url);
            result.put("status", "ok");
            return result;
        });
    }
}

/**
 * Main entry point.
 */
public class Main {
    public static void main(String[] args) {
        try {
            UserService service = new UserService();
            service.initialize();

            User user = service.createUser("1", "Test User");
            System.out.println("Created user: " + user.getDisplayName());

            long result = MathUtils.calculateFactorial(5);
            System.out.println("Factorial: " + result);
        } catch (ServiceException e) {
            e.printStackTrace();
        }
    }
}
