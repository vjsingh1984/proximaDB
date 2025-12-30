// Copyright 2025 Vijaykumar Singh
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package proximadb

import (
	"os"
	"time"
)

// Config holds the client configuration.
type Config struct {
	// URL is the ProximaDB server URL.
	URL string
	// APIKey is the optional API key for authentication.
	APIKey string
	// Protocol is the communication protocol (REST or gRPC).
	Protocol Protocol
	// Timeout is the request timeout.
	Timeout time.Duration
	// MaxRetries is the maximum number of retry attempts.
	MaxRetries int
	// RetryDelay is the initial delay between retries.
	RetryDelay time.Duration
	// MaxRetryDelay is the maximum delay between retries.
	MaxRetryDelay time.Duration
	// PoolSize is the connection pool size for gRPC.
	PoolSize int
	// TLS contains TLS configuration options.
	TLS *TLSConfig
	// Middlewares is the list of middlewares to apply to operations.
	Middlewares []Middleware
	// UserAgent is the custom user agent string.
	UserAgent string
	// EnableCompression enables request/response compression.
	EnableCompression bool
	// MaxIdleConns is the maximum number of idle connections.
	MaxIdleConns int
	// IdleConnTimeout is the idle connection timeout.
	IdleConnTimeout time.Duration
}

// TLSConfig holds TLS configuration options.
type TLSConfig struct {
	// CertFile is the path to the client certificate file.
	CertFile string
	// KeyFile is the path to the client key file.
	KeyFile string
	// CAFile is the path to the CA certificate file.
	CAFile string
	// SkipVerify skips server certificate verification (insecure).
	SkipVerify bool
}

// Option is a function that modifies the client configuration.
type Option func(*Config)

// WithURL sets the server URL.
func WithURL(url string) Option {
	return func(c *Config) {
		c.URL = url
	}
}

// WithAPIKey sets the API key for authentication.
func WithAPIKey(key string) Option {
	return func(c *Config) {
		c.APIKey = key
	}
}

// WithProtocol sets the communication protocol.
func WithProtocol(p Protocol) Option {
	return func(c *Config) {
		c.Protocol = p
	}
}

// WithTimeout sets the request timeout.
func WithTimeout(d time.Duration) Option {
	return func(c *Config) {
		c.Timeout = d
	}
}

// WithMaxRetries sets the maximum number of retry attempts.
func WithMaxRetries(n int) Option {
	return func(c *Config) {
		c.MaxRetries = n
	}
}

// WithRetryDelay sets the initial delay between retries.
func WithRetryDelay(d time.Duration) Option {
	return func(c *Config) {
		c.RetryDelay = d
	}
}

// WithPoolSize sets the connection pool size for gRPC.
func WithPoolSize(size int) Option {
	return func(c *Config) {
		c.PoolSize = size
	}
}

// WithTLS sets TLS configuration.
func WithTLS(tls *TLSConfig) Option {
	return func(c *Config) {
		c.TLS = tls
	}
}

// WithInsecureTLS enables TLS with certificate verification skipped.
func WithInsecureTLS() Option {
	return func(c *Config) {
		c.TLS = &TLSConfig{SkipVerify: true}
	}
}

// WithMiddleware adds a middleware to the configuration.
func WithMiddleware(m Middleware) Option {
	return func(c *Config) {
		c.Middlewares = append(c.Middlewares, m)
	}
}

// WithUserAgent sets a custom user agent string.
func WithUserAgent(ua string) Option {
	return func(c *Config) {
		c.UserAgent = ua
	}
}

// WithCompression enables request/response compression.
func WithCompression(enabled bool) Option {
	return func(c *Config) {
		c.EnableCompression = enabled
	}
}

// WithMaxIdleConns sets the maximum number of idle connections.
func WithMaxIdleConns(n int) Option {
	return func(c *Config) {
		c.MaxIdleConns = n
	}
}

// WithIdleConnTimeout sets the idle connection timeout.
func WithIdleConnTimeout(d time.Duration) Option {
	return func(c *Config) {
		c.IdleConnTimeout = d
	}
}

// WithMaxRetryDelay sets the maximum retry delay.
func WithMaxRetryDelay(d time.Duration) Option {
	return func(c *Config) {
		c.MaxRetryDelay = d
	}
}

// defaultConfig returns the default configuration.
func defaultConfig() *Config {
	return &Config{
		URL:             getEnvOrDefault("PROXIMADB_URL", "http://localhost:5678"),
		APIKey:          os.Getenv("PROXIMADB_API_KEY"),
		Protocol:        Protocol(getEnvOrDefault("PROXIMADB_PROTOCOL", string(ProtocolREST))),
		Timeout:         30 * time.Second,
		MaxRetries:      3,
		RetryDelay:      100 * time.Millisecond,
		MaxRetryDelay:   10 * time.Second,
		PoolSize:        10,
		UserAgent:       "proximadb-go/1.0.0",
		MaxIdleConns:    100,
		IdleConnTimeout: 90 * time.Second,
	}
}

// getEnvOrDefault returns the environment variable value or the default.
func getEnvOrDefault(key, defaultValue string) string {
	if value := os.Getenv(key); value != "" {
		return value
	}
	return defaultValue
}

// Validate validates the configuration.
func (c *Config) Validate() error {
	if c.URL == "" {
		return NewError(ErrCodeInvalidArgument, "URL is required")
	}
	if c.Timeout <= 0 {
		return NewError(ErrCodeInvalidArgument, "timeout must be positive")
	}
	if c.MaxRetries < 0 {
		return NewError(ErrCodeInvalidArgument, "max retries cannot be negative")
	}
	if c.PoolSize <= 0 {
		return NewError(ErrCodeInvalidArgument, "pool size must be positive")
	}
	return nil
}

// Clone creates a deep copy of the configuration.
func (c *Config) Clone() *Config {
	clone := *c
	if c.TLS != nil {
		tlsCopy := *c.TLS
		clone.TLS = &tlsCopy
	}
	return &clone
}
