/**
 * Sample JavaScript file for testing code chunking.
 *
 * This file contains various JavaScript constructs to test AST parsing.
 */

// Constants
const MAX_RETRIES = 3;
const DEFAULT_TIMEOUT = 30.0;

/**
 * Represents a user in the system.
 */
class User {
    constructor(id, name, email = null) {
        this.id = id;
        this.name = name;
        this.email = email;
    }

    getDisplayName() {
        return this.name || this.email || this.id;
    }

    setEmail(email) {
        this.email = email;
    }
}

/**
 * Base class for services.
 */
class BaseService {
    constructor(config) {
        this.config = config;
        this._initialized = false;
    }

    initialize() {
        this._initialized = true;
    }

    _validateConfig() {
        return Boolean(this.config);
    }
}

/**
 * Service for managing users.
 */
class UserService extends BaseService {
    constructor(config) {
        super(config);
        this.users = new Map();
    }

    createUser(id, name, email = null) {
        if (!id) {
            throw new Error('ID cannot be empty');
        }
        const user = new User(id, name, email);
        this.users.set(id, user);
        this._onUserCreated(user);
        return user;
    }

    getUser(id) {
        return this.users.get(id) || null;
    }

    deleteUser(id) {
        return this.users.delete(id);
    }

    _onUserCreated(user) {
        // Internal callback
    }
}

/**
 * Calculate factorial of n.
 * @param {number} n - The number to calculate factorial for
 * @returns {number} The factorial
 */
function calculateFactorial(n) {
    if (n <= 1) {
        return 1;
    }
    return n * calculateFactorial(n - 1);
}

/**
 * Fetch data from URL asynchronously.
 * @param {string} url - The URL to fetch
 * @param {number} timeout - Timeout in seconds
 * @returns {Promise<Object>} The fetched data
 */
async function fetchData(url, timeout = DEFAULT_TIMEOUT) {
    // Simulated async fetch
    return { url, status: 'ok' };
}

/**
 * Process items with optional validation.
 * @param {string[]} items - Items to process
 * @param {boolean} validate - Whether to validate
 * @returns {string[]} Processed items
 */
const processItems = (items, validate = true) => {
    let filtered = validate ? items.filter(item => item) : items;
    return filtered.map(item => item.trim().toLowerCase());
};

// Arrow function with destructuring
const getUserInfo = ({ id, name, email }) => ({
    displayId: id,
    displayName: name || email || 'Unknown',
});

// Generator function
function* idGenerator() {
    let id = 1;
    while (true) {
        yield `id_${id++}`;
    }
}

// Module exports
module.exports = {
    User,
    UserService,
    BaseService,
    calculateFactorial,
    fetchData,
    processItems,
    getUserInfo,
    idGenerator,
    MAX_RETRIES,
    DEFAULT_TIMEOUT,
};

// Main execution
function main() {
    const service = new UserService({ env: 'test' });
    service.initialize();

    const user = service.createUser('1', 'Test User', 'test@example.com');
    console.log(`Created user: ${user.getDisplayName()}`);

    const result = calculateFactorial(5);
    console.log(`Factorial: ${result}`);
}

if (require.main === module) {
    main();
}
