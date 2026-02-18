// Package sample provides sample Go code for testing code chunking.
//
// This package contains various Go constructs to test AST parsing.
package sample

import (
	"context"
	"errors"
	"fmt"
	"sync"
)

// MaxRetries is the maximum number of retries for operations
const MaxRetries = 3

// DefaultTimeout is the default timeout in seconds
const DefaultTimeout = 30.0

// Common errors
var (
	ErrNotFound     = errors.New("not found")
	ErrInvalidInput = errors.New("invalid input")
)

// User represents a user in the system
type User struct {
	ID    string
	Name  string
	Email string
}

// NewUser creates a new user
func NewUser(id, name string) *User {
	return &User{
		ID:   id,
		Name: name,
	}
}

// GetDisplayName returns the display name for the user
func (u *User) GetDisplayName() string {
	return u.Name
}

// SetEmail sets the user's email
func (u *User) SetEmail(email string) {
	u.Email = email
}

// Service defines the interface for services
type Service interface {
	Initialize(ctx context.Context) error
	IsReady() bool
}

// UserService manages users
type UserService struct {
	users       map[string]*User
	initialized bool
	mu          sync.RWMutex
}

// NewUserService creates a new UserService
func NewUserService() *UserService {
	return &UserService{
		users: make(map[string]*User),
	}
}

// Initialize initializes the service
func (s *UserService) Initialize(ctx context.Context) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.initialized = true
	return nil
}

// IsReady returns true if the service is ready
func (s *UserService) IsReady() bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.initialized
}

// CreateUser creates a new user
func (s *UserService) CreateUser(id, name string) (*User, error) {
	if id == "" {
		return nil, ErrInvalidInput
	}

	s.mu.Lock()
	defer s.mu.Unlock()

	user := NewUser(id, name)
	s.users[id] = user
	s.onUserCreated(user)

	return user, nil
}

// GetUser gets a user by ID
func (s *UserService) GetUser(id string) (*User, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	user, ok := s.users[id]
	if !ok {
		return nil, ErrNotFound
	}
	return user, nil
}

// DeleteUser deletes a user by ID
func (s *UserService) DeleteUser(id string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()

	if _, ok := s.users[id]; ok {
		delete(s.users, id)
		return true
	}
	return false
}

// onUserCreated is called when a user is created
func (s *UserService) onUserCreated(user *User) {
	// Internal callback
}

// CalculateFactorial calculates the factorial of n
func CalculateFactorial(n uint64) uint64 {
	if n <= 1 {
		return 1
	}
	return n * CalculateFactorial(n-1)
}

// FetchData fetches data from a URL
func FetchData(ctx context.Context, url string) (map[string]string, error) {
	result := map[string]string{
		"url":    url,
		"status": "ok",
	}
	return result, nil
}

// ProcessItems processes a list of items
func ProcessItems(items []string, validate bool) []string {
	var result []string

	for _, item := range items {
		if validate && item == "" {
			continue
		}
		result = append(result, item)
	}

	return result
}

// Config holds service configuration
type Config struct {
	Env      string
	Debug    bool
	MaxConns int
}

// Validate validates the configuration
func (c *Config) Validate() error {
	if c.Env == "" {
		return ErrInvalidInput
	}
	return nil
}

func main() {
	ctx := context.Background()

	service := NewUserService()
	if err := service.Initialize(ctx); err != nil {
		panic(err)
	}

	user, err := service.CreateUser("1", "Test User")
	if err != nil {
		panic(err)
	}
	fmt.Printf("Created user: %s\n", user.GetDisplayName())

	result := CalculateFactorial(5)
	fmt.Printf("Factorial: %d\n", result)
}
