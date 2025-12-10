/**
 * Sample C# file for testing code chunking.
 *
 * This file contains various C# constructs to test AST parsing.
 */

using System;
using System.Collections.Generic;
using System.Threading.Tasks;

namespace Sample
{
    /// <summary>
    /// Constants for the application.
    /// </summary>
    public static class Constants
    {
        public const int MaxRetries = 3;
        public const double DefaultTimeout = 30.0;
    }

    /// <summary>
    /// Represents a user in the system.
    /// </summary>
    public class User
    {
        public string Id { get; }
        public string Name { get; }
        public string? Email { get; set; }

        public User(string id, string name)
        {
            Id = id;
            Name = name;
        }

        public User(string id, string name, string email) : this(id, name)
        {
            Email = email;
        }

        public string GetDisplayName()
        {
            return Name ?? Email ?? Id;
        }
    }

    /// <summary>
    /// Interface for services.
    /// </summary>
    public interface IService
    {
        Task InitializeAsync();
        bool IsReady { get; }
    }

    /// <summary>
    /// Custom exception for service errors.
    /// </summary>
    public class ServiceException : Exception
    {
        public ServiceException(string message) : base(message) { }
        public ServiceException(string message, Exception inner) : base(message, inner) { }
    }

    /// <summary>
    /// Service for managing users.
    /// </summary>
    public class UserService : IService
    {
        private readonly Dictionary<string, User> _users = new();
        private bool _initialized;

        public bool IsReady => _initialized;

        public async Task InitializeAsync()
        {
            await Task.Delay(10); // Simulate async initialization
            _initialized = true;
        }

        public User CreateUser(string id, string name, string? email = null)
        {
            if (string.IsNullOrEmpty(id))
            {
                throw new ServiceException("ID cannot be empty");
            }

            var user = email != null
                ? new User(id, name, email)
                : new User(id, name);

            _users[id] = user;
            OnUserCreated(user);
            return user;
        }

        public User? GetUser(string id)
        {
            return _users.TryGetValue(id, out var user) ? user : null;
        }

        public bool DeleteUser(string id)
        {
            return _users.Remove(id);
        }

        private void OnUserCreated(User user)
        {
            // Internal callback
        }
    }

    /// <summary>
    /// Generic container class.
    /// </summary>
    public class Container<T>
    {
        public T Value { get; set; }

        public Container(T value)
        {
            Value = value;
        }
    }

    /// <summary>
    /// Record type for immutable data.
    /// </summary>
    public record UserRecord(string Id, string Name, string? Email = null);

    /// <summary>
    /// Struct for value types.
    /// </summary>
    public struct Point
    {
        public double X { get; init; }
        public double Y { get; init; }

        public double Distance => Math.Sqrt(X * X + Y * Y);
    }

    /// <summary>
    /// Enum for service status.
    /// </summary>
    public enum ServiceStatus
    {
        Pending,
        Running,
        Stopped,
        Error
    }

    /// <summary>
    /// Utility class with static methods.
    /// </summary>
    public static class MathUtils
    {
        /// <summary>
        /// Calculate factorial of n.
        /// </summary>
        public static long CalculateFactorial(int n)
        {
            if (n <= 1) return 1;
            return n * CalculateFactorial(n - 1);
        }

        /// <summary>
        /// Fetch data asynchronously.
        /// </summary>
        public static async Task<Dictionary<string, string>> FetchDataAsync(string url)
        {
            await Task.Delay(10); // Simulate network call
            return new Dictionary<string, string>
            {
                ["url"] = url,
                ["status"] = "ok"
            };
        }

        /// <summary>
        /// Process items with optional validation.
        /// </summary>
        public static List<string> ProcessItems(List<string> items, bool validate = true)
        {
            var result = new List<string>();
            foreach (var item in items)
            {
                if (validate && string.IsNullOrEmpty(item))
                    continue;
                result.Add(item.Trim().ToLower());
            }
            return result;
        }
    }

    /// <summary>
    /// Extension methods for strings.
    /// </summary>
    public static class StringExtensions
    {
        public static string Truncate(this string value, int maxLength)
        {
            if (string.IsNullOrEmpty(value)) return value;
            return value.Length <= maxLength ? value : value[..maxLength];
        }
    }

    /// <summary>
    /// Main program entry point.
    /// </summary>
    public class Program
    {
        public static async Task Main(string[] args)
        {
            var service = new UserService();
            await service.InitializeAsync();

            var user = service.CreateUser("1", "Test User", "test@example.com");
            Console.WriteLine($"Created user: {user.GetDisplayName()}");

            var result = MathUtils.CalculateFactorial(5);
            Console.WriteLine($"Factorial: {result}");
        }
    }
}
