<?php
/**
 * Sample PHP file for testing code chunking.
 *
 * This file contains various PHP constructs to test AST parsing.
 */

declare(strict_types=1);

namespace Sample;

use Exception;
use InvalidArgumentException;

// Constants
define('MAX_RETRIES', 3);
const DEFAULT_TIMEOUT = 30.0;

/**
 * Custom exception for service errors.
 */
class ServiceException extends Exception
{
    private ?string $errorCode;

    public function __construct(string $message, ?string $errorCode = null, int $code = 0, ?Exception $previous = null)
    {
        parent::__construct($message, $code, $previous);
        $this->errorCode = $errorCode;
    }

    public function getErrorCode(): ?string
    {
        return $this->errorCode;
    }
}

/**
 * Represents a user in the system.
 */
class User
{
    private string $id;
    private string $name;
    private ?string $email;

    public function __construct(string $id, string $name, ?string $email = null)
    {
        $this->id = $id;
        $this->name = $name;
        $this->email = $email;
    }

    public function getId(): string
    {
        return $this->id;
    }

    public function getName(): string
    {
        return $this->name;
    }

    public function getEmail(): ?string
    {
        return $this->email;
    }

    public function setEmail(string $email): void
    {
        $this->email = $email;
    }

    public function getDisplayName(): string
    {
        return $this->name ?: ($this->email ?: $this->id);
    }

    public function toArray(): array
    {
        return [
            'id' => $this->id,
            'name' => $this->name,
            'email' => $this->email,
        ];
    }
}

/**
 * Interface for services.
 */
interface ServiceInterface
{
    public function initialize(): void;
    public function isReady(): bool;
}

/**
 * Trait for common functionality.
 */
trait Loggable
{
    protected function log(string $message): void
    {
        echo "[LOG] $message\n";
    }
}

/**
 * Base class for services.
 */
abstract class BaseService implements ServiceInterface
{
    use Loggable;

    protected array $config;
    protected bool $initialized = false;

    public function __construct(array $config = [])
    {
        $this->config = $config;
    }

    public function initialize(): void
    {
        $this->initialized = true;
    }

    public function isReady(): bool
    {
        return $this->initialized;
    }

    protected function validateConfig(): bool
    {
        return !empty($this->config);
    }
}

/**
 * Service for managing users.
 */
class UserService extends BaseService
{
    private array $users = [];

    public function createUser(string $id, string $name, ?string $email = null): User
    {
        if (empty($id)) {
            throw new ServiceException('ID cannot be empty', 'INVALID_ID');
        }

        $user = new User($id, $name, $email);
        $this->users[$id] = $user;
        $this->onUserCreated($user);
        return $user;
    }

    public function getUser(string $id): ?User
    {
        return $this->users[$id] ?? null;
    }

    public function deleteUser(string $id): bool
    {
        if (isset($this->users[$id])) {
            unset($this->users[$id]);
            return true;
        }
        return false;
    }

    public function getUsers(): array
    {
        return array_values($this->users);
    }

    private function onUserCreated(User $user): void
    {
        // Internal callback
    }
}

/**
 * Calculate factorial of n.
 */
function calculateFactorial(int $n): int
{
    if ($n <= 1) {
        return 1;
    }
    return $n * calculateFactorial($n - 1);
}

/**
 * Fetch data from URL (simulated).
 */
function fetchData(string $url, float $timeout = DEFAULT_TIMEOUT): array
{
    // Simulated fetch
    return [
        'url' => $url,
        'status' => 'ok',
        'timeout' => $timeout,
    ];
}

/**
 * Process items with optional validation.
 */
function processItems(array $items, bool $validate = true): array
{
    if ($validate) {
        $items = array_filter($items, fn($item) => !empty($item));
    }
    return array_map(fn($item) => strtolower(trim($item)), $items);
}

// Arrow function (PHP 7.4+)
$doubleValue = fn(int $x): int => $x * 2;

// Main execution
if (php_sapi_name() === 'cli' && basename(__FILE__) === basename($argv[0] ?? '')) {
    $service = new UserService(['env' => 'test']);
    $service->initialize();

    $user = $service->createUser('1', 'Test User', 'test@example.com');
    echo "Created user: " . $user->getDisplayName() . "\n";

    $result = calculateFactorial(5);
    echo "Factorial: $result\n";
}
