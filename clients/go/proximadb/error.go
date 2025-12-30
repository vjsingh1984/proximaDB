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
	"fmt"
)

// ErrorCode represents a ProximaDB error code.
type ErrorCode string

const (
	// ErrCodeConnection indicates a connection error.
	ErrCodeConnection ErrorCode = "CONNECTION_ERROR"
	// ErrCodeTimeout indicates a timeout error.
	ErrCodeTimeout ErrorCode = "TIMEOUT"
	// ErrCodeNotFound indicates a resource was not found.
	ErrCodeNotFound ErrorCode = "NOT_FOUND"
	// ErrCodeAlreadyExists indicates a resource already exists.
	ErrCodeAlreadyExists ErrorCode = "ALREADY_EXISTS"
	// ErrCodeInvalidArgument indicates an invalid argument.
	ErrCodeInvalidArgument ErrorCode = "INVALID_ARGUMENT"
	// ErrCodeDimensionMismatch indicates a vector dimension mismatch.
	ErrCodeDimensionMismatch ErrorCode = "DIMENSION_MISMATCH"
	// ErrCodeRateLimited indicates rate limiting.
	ErrCodeRateLimited ErrorCode = "RATE_LIMITED"
	// ErrCodeInternal indicates an internal server error.
	ErrCodeInternal ErrorCode = "INTERNAL_ERROR"
	// ErrCodeUnavailable indicates the service is unavailable.
	ErrCodeUnavailable ErrorCode = "UNAVAILABLE"
)

// ProximaDBError represents an error from ProximaDB operations.
type ProximaDBError struct {
	// Code is the error code.
	Code ErrorCode
	// Message is the error message.
	Message string
	// Details contains additional error details.
	Details map[string]interface{}
	// Cause is the underlying error.
	Cause error
}

// Error implements the error interface.
func (e *ProximaDBError) Error() string {
	if e.Cause != nil {
		return fmt.Sprintf("[%s] %s: %v", e.Code, e.Message, e.Cause)
	}
	return fmt.Sprintf("[%s] %s", e.Code, e.Message)
}

// Unwrap returns the underlying error.
func (e *ProximaDBError) Unwrap() error {
	return e.Cause
}

// Is checks if the error matches a target error code.
func (e *ProximaDBError) Is(target error) bool {
	if t, ok := target.(*ProximaDBError); ok {
		return e.Code == t.Code
	}
	return false
}

// NewError creates a new ProximaDBError.
func NewError(code ErrorCode, message string) *ProximaDBError {
	return &ProximaDBError{
		Code:    code,
		Message: message,
	}
}

// WrapError wraps an error with a ProximaDBError.
func WrapError(code ErrorCode, message string, cause error) *ProximaDBError {
	return &ProximaDBError{
		Code:    code,
		Message: message,
		Cause:   cause,
	}
}

// IsNotFound checks if the error is a not found error.
func IsNotFound(err error) bool {
	if e, ok := err.(*ProximaDBError); ok {
		return e.Code == ErrCodeNotFound
	}
	return false
}

// IsAlreadyExists checks if the error is an already exists error.
func IsAlreadyExists(err error) bool {
	if e, ok := err.(*ProximaDBError); ok {
		return e.Code == ErrCodeAlreadyExists
	}
	return false
}

// IsTimeout checks if the error is a timeout error.
func IsTimeout(err error) bool {
	if e, ok := err.(*ProximaDBError); ok {
		return e.Code == ErrCodeTimeout
	}
	return false
}

// IsRateLimited checks if the error is a rate limit error.
func IsRateLimited(err error) bool {
	if e, ok := err.(*ProximaDBError); ok {
		return e.Code == ErrCodeRateLimited
	}
	return false
}

// IsDimensionMismatch checks if the error is a dimension mismatch error.
func IsDimensionMismatch(err error) bool {
	if e, ok := err.(*ProximaDBError); ok {
		return e.Code == ErrCodeDimensionMismatch
	}
	return false
}

// IsConnectionError checks if the error is a connection error.
func IsConnectionError(err error) bool {
	if e, ok := err.(*ProximaDBError); ok {
		return e.Code == ErrCodeConnection
	}
	return false
}

// IsRetryable checks if the error is retryable.
func IsRetryable(err error) bool {
	if e, ok := err.(*ProximaDBError); ok {
		switch e.Code {
		case ErrCodeTimeout, ErrCodeRateLimited, ErrCodeUnavailable:
			return true
		}
	}
	return false
}
