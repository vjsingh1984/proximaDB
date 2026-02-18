# frozen_string_literal: true

# Sample Ruby file for testing code chunking.
#
# This file contains various Ruby constructs to test AST parsing.

require 'json'
require 'ostruct'

# Constants
MAX_RETRIES = 3
DEFAULT_TIMEOUT = 30.0

# Custom error class
class ServiceError < StandardError
  attr_reader :code

  def initialize(message, code = nil)
    super(message)
    @code = code
  end
end

# Represents a user in the system
class User
  attr_reader :id, :name
  attr_accessor :email

  def initialize(id, name, email = nil)
    @id = id
    @name = name
    @email = email
  end

  def display_name
    name || email || id
  end

  def to_h
    { id: id, name: name, email: email }
  end

  def to_json(*_args)
    to_h.to_json
  end
end

# Base class for services
class BaseService
  attr_reader :config

  def initialize(config = {})
    @config = config
    @initialized = false
  end

  def initialize!
    @initialized = true
  end

  def ready?
    @initialized
  end

  protected

  def validate_config
    !config.empty?
  end
end

# Service for managing users
class UserService < BaseService
  def initialize(config = {})
    super
    @users = {}
  end

  def create_user(id, name, email = nil)
    raise ServiceError.new('ID cannot be empty') if id.nil? || id.empty?

    user = User.new(id, name, email)
    @users[id] = user
    on_user_created(user)
    user
  end

  def get_user(id)
    @users[id]
  end

  def delete_user(id)
    !!@users.delete(id)
  end

  def each_user(&block)
    @users.values.each(&block)
  end

  private

  def on_user_created(user)
    # Internal callback
  end
end

# Module with utility methods
module MathUtils
  module_function

  def calculate_factorial(n)
    return 1 if n <= 1
    n * calculate_factorial(n - 1)
  end

  def fibonacci(n)
    return n if n <= 1
    fibonacci(n - 1) + fibonacci(n - 2)
  end
end

# Mixin module
module Serializable
  def serialize
    to_h.to_json
  end

  def self.included(base)
    base.extend(ClassMethods)
  end

  module ClassMethods
    def from_json(json)
      data = JSON.parse(json, symbolize_names: true)
      new(**data)
    end
  end
end

# Fetch data asynchronously (simulated)
def fetch_data(url, timeout: DEFAULT_TIMEOUT)
  # Simulated fetch
  { url: url, status: 'ok', timeout: timeout }
end

# Process items with optional validation
def process_items(items, validate: true)
  filtered = validate ? items.compact.reject(&:empty?) : items
  filtered.map { |item| item.strip.downcase }
end

# Block and yield example
def with_retry(max_retries = MAX_RETRIES)
  retries = 0
  begin
    yield
  rescue StandardError => e
    retries += 1
    retry if retries < max_retries
    raise e
  end
end

# Struct definition
UserStruct = Struct.new(:id, :name, :email) do
  def display_name
    name || email || id
  end
end

# Main execution
if __FILE__ == $PROGRAM_NAME
  service = UserService.new(env: 'test')
  service.initialize!

  user = service.create_user('1', 'Test User', 'test@example.com')
  puts "Created user: #{user.display_name}"

  result = MathUtils.calculate_factorial(5)
  puts "Factorial: #{result}"
end
