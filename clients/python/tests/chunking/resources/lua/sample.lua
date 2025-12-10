--[[
Sample Lua file for testing code chunking.

This file contains various Lua constructs to test AST parsing.
]]

-- Constants
local MAX_RETRIES = 3
local DEFAULT_TIMEOUT = 30.0

-- User class
local User = {}
User.__index = User

--- Create a new User instance.
-- @param id string User ID
-- @param name string User name
-- @param email string|nil Optional email
-- @return User New User instance
function User.new(id, name, email)
    local self = setmetatable({}, User)
    self.id = id
    self.name = name
    self.email = email
    return self
end

--- Get the display name for the user.
-- @return string Display name
function User:getDisplayName()
    if self.name and self.name ~= "" then
        return self.name
    elseif self.email then
        return self.email
    else
        return self.id
    end
end

--- Set the user's email.
-- @param email string Email address
function User:setEmail(email)
    self.email = email
end

--- Convert user to table.
-- @return table User data
function User:toTable()
    return {
        id = self.id,
        name = self.name,
        email = self.email
    }
end

-- UserService class
local UserService = {}
UserService.__index = UserService

--- Create a new UserService instance.
-- @param config table|nil Configuration
-- @return UserService New UserService instance
function UserService.new(config)
    local self = setmetatable({}, UserService)
    self.users = {}
    self.initialized = false
    self.config = config or {}
    return self
end

--- Initialize the service.
function UserService:initialize()
    self.initialized = true
end

--- Check if service is ready.
-- @return boolean Ready status
function UserService:isReady()
    return self.initialized
end

--- Create a new user.
-- @param id string User ID
-- @param name string User name
-- @param email string|nil Optional email
-- @return User Created user
function UserService:createUser(id, name, email)
    assert(id and id ~= "", "ID cannot be empty")

    local user = User.new(id, name, email)
    self.users[id] = user
    self:_onUserCreated(user)
    return user
end

--- Get a user by ID.
-- @param id string User ID
-- @return User|nil User if found
function UserService:getUser(id)
    return self.users[id]
end

--- Delete a user by ID.
-- @param id string User ID
-- @return boolean True if deleted
function UserService:deleteUser(id)
    if self.users[id] then
        self.users[id] = nil
        return true
    end
    return false
end

--- Get all users.
-- @return table List of users
function UserService:getAllUsers()
    local result = {}
    for _, user in pairs(self.users) do
        table.insert(result, user)
    end
    return result
end

-- Private callback
function UserService:_onUserCreated(user)
    -- Internal callback
end

--- Calculate factorial of n.
-- @param n number The number
-- @return number Factorial
local function calculateFactorial(n)
    if n <= 1 then
        return 1
    end
    return n * calculateFactorial(n - 1)
end

--- Fetch data from URL (simulated).
-- @param url string URL to fetch
-- @param timeout number|nil Timeout in seconds
-- @return table Response data
local function fetchData(url, timeout)
    timeout = timeout or DEFAULT_TIMEOUT
    return {
        url = url,
        status = "ok",
        timeout = timeout
    }
end

--- Process items with optional validation.
-- @param items table List of items
-- @param validate boolean|nil Whether to validate
-- @return table Processed items
local function processItems(items, validate)
    if validate == nil then
        validate = true
    end

    local result = {}
    for _, item in ipairs(items) do
        if not validate or (item and item ~= "") then
            local processed = string.lower(string.gsub(item, "^%s*(.-)%s*$", "%1"))
            table.insert(result, processed)
        end
    end
    return result
end

--- Execute block with retry.
-- @param maxRetries number Maximum retries
-- @param block function Block to execute
-- @return any Block result
local function withRetry(maxRetries, block)
    maxRetries = maxRetries or MAX_RETRIES
    local lastError

    for i = 1, maxRetries do
        local success, result = pcall(block)
        if success then
            return result
        end
        lastError = result
    end

    error("Max retries exceeded: " .. tostring(lastError))
end

--- Main entry point.
local function main()
    local service = UserService.new({ env = "test" })
    service:initialize()

    local user = service:createUser("1", "Test User", "test@example.com")
    print("Created user: " .. user:getDisplayName())

    local result = calculateFactorial(5)
    print("Factorial: " .. result)
end

-- Export module
local M = {
    User = User,
    UserService = UserService,
    calculateFactorial = calculateFactorial,
    fetchData = fetchData,
    processItems = processItems,
    withRetry = withRetry,
    MAX_RETRIES = MAX_RETRIES,
    DEFAULT_TIMEOUT = DEFAULT_TIMEOUT,
}

-- Run main if executed directly
if not pcall(debug.getlocal, 4, 1) then
    main()
end

return M
