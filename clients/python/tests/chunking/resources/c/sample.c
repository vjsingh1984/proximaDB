/**
 * Sample C file for testing code chunking.
 *
 * This file contains various C constructs to test AST parsing.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>

/* Constants */
#define MAX_RETRIES 3
#define DEFAULT_TIMEOUT 30.0
#define MAX_NAME_LENGTH 256

/* Error codes */
typedef enum {
    ERR_SUCCESS = 0,
    ERR_NOT_FOUND = -1,
    ERR_INVALID_INPUT = -2,
    ERR_OUT_OF_MEMORY = -3
} ErrorCode;

/* User structure */
typedef struct User {
    char* id;
    char* name;
    char* email;
} User;

/* UserService structure */
typedef struct UserService {
    User** users;
    size_t count;
    size_t capacity;
    bool initialized;
} UserService;

/**
 * Create a new user.
 *
 * @param id User ID
 * @param name User name
 * @return Pointer to new User or NULL on failure
 */
User* user_create(const char* id, const char* name) {
    if (id == NULL || name == NULL) {
        return NULL;
    }

    User* user = (User*)malloc(sizeof(User));
    if (user == NULL) {
        return NULL;
    }

    user->id = strdup(id);
    user->name = strdup(name);
    user->email = NULL;

    return user;
}

/**
 * Free a user.
 */
void user_free(User* user) {
    if (user != NULL) {
        free(user->id);
        free(user->name);
        free(user->email);
        free(user);
    }
}

/**
 * Get the display name for a user.
 */
const char* user_get_display_name(const User* user) {
    if (user == NULL) {
        return NULL;
    }
    if (user->name != NULL) {
        return user->name;
    }
    if (user->email != NULL) {
        return user->email;
    }
    return user->id;
}

/**
 * Set the user's email.
 */
void user_set_email(User* user, const char* email) {
    if (user != NULL && email != NULL) {
        free(user->email);
        user->email = strdup(email);
    }
}

/**
 * Create a new user service.
 */
UserService* user_service_create(void) {
    UserService* service = (UserService*)malloc(sizeof(UserService));
    if (service == NULL) {
        return NULL;
    }

    service->capacity = 16;
    service->users = (User**)calloc(service->capacity, sizeof(User*));
    if (service->users == NULL) {
        free(service);
        return NULL;
    }

    service->count = 0;
    service->initialized = false;

    return service;
}

/**
 * Initialize the service.
 */
ErrorCode user_service_initialize(UserService* service) {
    if (service == NULL) {
        return ERR_INVALID_INPUT;
    }
    service->initialized = true;
    return ERR_SUCCESS;
}

/**
 * Check if service is ready.
 */
bool user_service_is_ready(const UserService* service) {
    return service != NULL && service->initialized;
}

/**
 * Add a user to the service.
 */
ErrorCode user_service_add_user(UserService* service, const char* id, const char* name) {
    if (service == NULL || id == NULL) {
        return ERR_INVALID_INPUT;
    }

    /* Expand capacity if needed */
    if (service->count >= service->capacity) {
        size_t new_capacity = service->capacity * 2;
        User** new_users = (User**)realloc(service->users, new_capacity * sizeof(User*));
        if (new_users == NULL) {
            return ERR_OUT_OF_MEMORY;
        }
        service->users = new_users;
        service->capacity = new_capacity;
    }

    User* user = user_create(id, name);
    if (user == NULL) {
        return ERR_OUT_OF_MEMORY;
    }

    service->users[service->count++] = user;
    return ERR_SUCCESS;
}

/**
 * Get a user by ID.
 */
User* user_service_get_user(const UserService* service, const char* id) {
    if (service == NULL || id == NULL) {
        return NULL;
    }

    for (size_t i = 0; i < service->count; i++) {
        if (strcmp(service->users[i]->id, id) == 0) {
            return service->users[i];
        }
    }

    return NULL;
}

/**
 * Free the user service.
 */
void user_service_free(UserService* service) {
    if (service != NULL) {
        for (size_t i = 0; i < service->count; i++) {
            user_free(service->users[i]);
        }
        free(service->users);
        free(service);
    }
}

/**
 * Calculate factorial of n.
 */
unsigned long calculate_factorial(unsigned int n) {
    if (n <= 1) {
        return 1;
    }
    return n * calculate_factorial(n - 1);
}

/**
 * Process items callback type.
 */
typedef void (*ItemCallback)(const char* item, void* context);

/**
 * Process items with a callback.
 */
void process_items(const char** items, size_t count, ItemCallback callback, void* context) {
    for (size_t i = 0; i < count; i++) {
        if (items[i] != NULL) {
            callback(items[i], context);
        }
    }
}

/* Static helper function */
static void print_item(const char* item, void* context) {
    printf("Item: %s\n", item);
}

/**
 * Main entry point.
 */
int main(int argc, char* argv[]) {
    UserService* service = user_service_create();
    if (service == NULL) {
        fprintf(stderr, "Failed to create service\n");
        return 1;
    }

    if (user_service_initialize(service) != ERR_SUCCESS) {
        fprintf(stderr, "Failed to initialize service\n");
        user_service_free(service);
        return 1;
    }

    user_service_add_user(service, "1", "Test User");
    User* user = user_service_get_user(service, "1");
    if (user != NULL) {
        printf("Created user: %s\n", user_get_display_name(user));
    }

    unsigned long result = calculate_factorial(5);
    printf("Factorial: %lu\n", result);

    user_service_free(service);
    return 0;
}
