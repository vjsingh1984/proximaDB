#!/bin/bash
#
# Sample Bash file for testing code chunking.
#
# This file contains various Bash constructs to test AST parsing.

# Constants
readonly MAX_RETRIES=3
readonly DEFAULT_TIMEOUT=30

# Global variables
declare -A USERS
INITIALIZED=false

# Error codes
readonly ERR_SUCCESS=0
readonly ERR_INVALID_INPUT=1
readonly ERR_NOT_FOUND=2

#######################################
# Print an error message to stderr.
# Arguments:
#   Message to print
# Returns:
#   None
#######################################
log_error() {
    echo "[ERROR] $1" >&2
}

#######################################
# Print an info message.
# Arguments:
#   Message to print
# Returns:
#   None
#######################################
log_info() {
    echo "[INFO] $1"
}

#######################################
# Initialize the service.
# Globals:
#   INITIALIZED
# Arguments:
#   None
# Returns:
#   0 on success
#######################################
initialize() {
    INITIALIZED=true
    log_info "Service initialized"
    return $ERR_SUCCESS
}

#######################################
# Check if service is ready.
# Globals:
#   INITIALIZED
# Arguments:
#   None
# Returns:
#   0 if ready, 1 otherwise
#######################################
is_ready() {
    if [[ "$INITIALIZED" == "true" ]]; then
        return 0
    else
        return 1
    fi
}

#######################################
# Create a new user.
# Globals:
#   USERS
# Arguments:
#   id - User ID
#   name - User name
#   email - Optional email
# Returns:
#   0 on success, 1 on error
#######################################
create_user() {
    local id="$1"
    local name="$2"
    local email="${3:-}"

    if [[ -z "$id" ]]; then
        log_error "ID cannot be empty"
        return $ERR_INVALID_INPUT
    fi

    # Store user as JSON-like string
    USERS["$id"]="name:$name,email:$email"
    on_user_created "$id" "$name"

    return $ERR_SUCCESS
}

#######################################
# Get a user by ID.
# Globals:
#   USERS
# Arguments:
#   id - User ID
# Outputs:
#   User data to stdout
# Returns:
#   0 if found, 2 if not found
#######################################
get_user() {
    local id="$1"

    if [[ -n "${USERS[$id]:-}" ]]; then
        echo "${USERS[$id]}"
        return $ERR_SUCCESS
    else
        return $ERR_NOT_FOUND
    fi
}

#######################################
# Delete a user by ID.
# Globals:
#   USERS
# Arguments:
#   id - User ID
# Returns:
#   0 if deleted, 2 if not found
#######################################
delete_user() {
    local id="$1"

    if [[ -n "${USERS[$id]:-}" ]]; then
        unset "USERS[$id]"
        return $ERR_SUCCESS
    else
        return $ERR_NOT_FOUND
    fi
}

# Private callback function
on_user_created() {
    local id="$1"
    local name="$2"
    # Internal callback
}

#######################################
# Calculate factorial of n.
# Arguments:
#   n - The number
# Outputs:
#   Factorial to stdout
# Returns:
#   0 on success
#######################################
calculate_factorial() {
    local n="$1"

    if (( n <= 1 )); then
        echo 1
    else
        local sub_result
        sub_result=$(calculate_factorial $((n - 1)))
        echo $((n * sub_result))
    fi
}

#######################################
# Fetch data from URL.
# Arguments:
#   url - The URL to fetch
#   timeout - Optional timeout (default: DEFAULT_TIMEOUT)
# Outputs:
#   JSON-like response to stdout
# Returns:
#   0 on success
#######################################
fetch_data() {
    local url="$1"
    local timeout="${2:-$DEFAULT_TIMEOUT}"

    # Simulated fetch
    echo "{\"url\": \"$url\", \"status\": \"ok\", \"timeout\": $timeout}"
    return $ERR_SUCCESS
}

#######################################
# Process items.
# Arguments:
#   validate - Whether to validate (true/false)
#   items... - Items to process
# Outputs:
#   Processed items to stdout
# Returns:
#   0 on success
#######################################
process_items() {
    local validate="$1"
    shift
    local items=("$@")

    for item in "${items[@]}"; do
        if [[ "$validate" == "true" && -z "$item" ]]; then
            continue
        fi
        # Trim and lowercase
        echo "$item" | tr '[:upper:]' '[:lower:]' | xargs
    done
}

#######################################
# Retry a command with exponential backoff.
# Arguments:
#   max_retries - Maximum number of retries
#   command... - Command to run
# Returns:
#   Exit code of the command
#######################################
with_retry() {
    local max_retries="$1"
    shift
    local cmd=("$@")
    local retry=0
    local exit_code

    while (( retry < max_retries )); do
        if "${cmd[@]}"; then
            return 0
        fi
        exit_code=$?
        (( retry++ ))
        sleep $((2 ** retry))
    done

    return $exit_code
}

# Main function
main() {
    initialize

    if ! is_ready; then
        log_error "Service not ready"
        exit 1
    fi

    create_user "1" "Test User" "test@example.com"

    if user=$(get_user "1"); then
        log_info "Created user: $user"
    fi

    result=$(calculate_factorial 5)
    log_info "Factorial: $result"
}

# Run main if script is executed directly
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
