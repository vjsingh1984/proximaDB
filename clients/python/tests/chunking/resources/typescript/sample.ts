/**
 * Sample TypeScript file for testing code chunking.
 *
 * This file contains various TypeScript constructs to test AST parsing.
 */

// Constants
const MAX_RETRIES: number = 3;
const DEFAULT_TIMEOUT: number = 30.0;

// Type definitions
type UserId = string;
type Metadata = Record<string, unknown>;

// Interface definitions
interface IUser {
    id: UserId;
    name: string;
    email?: string;
}

interface IService {
    initialize(): Promise<void>;
    isReady(): boolean;
}

interface IUserService extends IService {
    createUser(id: string, name: string, email?: string): User;
    getUser(id: string): User | null;
    deleteUser(id: string): boolean;
}

// Enum definition
enum ServiceStatus {
    PENDING = 'pending',
    RUNNING = 'running',
    STOPPED = 'stopped',
    ERROR = 'error',
}

/**
 * Represents a user in the system.
 */
class User implements IUser {
    public id: UserId;
    public name: string;
    public email?: string;

    constructor(id: UserId, name: string, email?: string) {
        this.id = id;
        this.name = name;
        this.email = email;
    }

    public getDisplayName(): string {
        return this.name || this.email || this.id;
    }

    public setEmail(email: string): void {
        this.email = email;
    }
}

/**
 * Base class for services.
 */
abstract class BaseService implements IService {
    protected config: Metadata;
    protected _initialized: boolean = false;
    protected status: ServiceStatus = ServiceStatus.PENDING;

    constructor(config: Metadata) {
        this.config = config;
    }

    public async initialize(): Promise<void> {
        this._initialized = true;
        this.status = ServiceStatus.RUNNING;
    }

    public isReady(): boolean {
        return this._initialized;
    }

    protected validateConfig(): boolean {
        return Boolean(this.config);
    }
}

/**
 * Service for managing users.
 */
class UserService extends BaseService implements IUserService {
    private users: Map<string, User> = new Map();

    constructor(config: Metadata) {
        super(config);
    }

    public createUser(id: string, name: string, email?: string): User {
        if (!id) {
            throw new Error('ID cannot be empty');
        }
        const user = new User(id, name, email);
        this.users.set(id, user);
        this.onUserCreated(user);
        return user;
    }

    public getUser(id: string): User | null {
        return this.users.get(id) || null;
    }

    public deleteUser(id: string): boolean {
        return this.users.delete(id);
    }

    private onUserCreated(user: User): void {
        // Internal callback
    }
}

/**
 * Calculate factorial of n.
 */
function calculateFactorial(n: number): number {
    if (n <= 1) {
        return 1;
    }
    return n * calculateFactorial(n - 1);
}

/**
 * Fetch data from URL asynchronously.
 */
async function fetchData(url: string, timeout: number = DEFAULT_TIMEOUT): Promise<Metadata> {
    // Simulated async fetch
    return { url, status: 'ok' };
}

/**
 * Process items with optional validation.
 */
const processItems = (items: string[], validate: boolean = true): string[] => {
    const filtered = validate ? items.filter(item => item) : items;
    return filtered.map(item => item.trim().toLowerCase());
};

// Generic function
function identity<T>(value: T): T {
    return value;
}

// Generic class
class Container<T> {
    private value: T;

    constructor(value: T) {
        this.value = value;
    }

    public getValue(): T {
        return this.value;
    }

    public setValue(value: T): void {
        this.value = value;
    }
}

// Type guard
function isUser(obj: unknown): obj is User {
    return obj instanceof User;
}

// Decorator (experimental)
function logged(target: any, propertyKey: string, descriptor: PropertyDescriptor) {
    const originalMethod = descriptor.value;
    descriptor.value = function (...args: any[]) {
        console.log(`Calling ${propertyKey} with`, args);
        return originalMethod.apply(this, args);
    };
    return descriptor;
}

// Main execution
async function main(): Promise<void> {
    const service = new UserService({ env: 'test' });
    await service.initialize();

    const user = service.createUser('1', 'Test User', 'test@example.com');
    console.log(`Created user: ${user.getDisplayName()}`);

    const result = calculateFactorial(5);
    console.log(`Factorial: ${result}`);
}

export {
    User,
    UserService,
    BaseService,
    calculateFactorial,
    fetchData,
    processItems,
    identity,
    Container,
    isUser,
    ServiceStatus,
    MAX_RETRIES,
    DEFAULT_TIMEOUT,
};
export type { IUser, IService, IUserService, UserId, Metadata };
