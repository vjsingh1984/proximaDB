# Sample Elixir file for testing code chunking.
#
# This module contains various Elixir constructs to test AST parsing.

defmodule Sample do
  @moduledoc """
  Sample module for testing code chunking.

  This module provides user management functionality
  and various utility functions.
  """

  # Module attributes (constants)
  @max_retries 3
  @default_timeout 30.0

  # Custom exceptions
  defmodule ServiceError do
    @moduledoc "Custom exception for service errors"
    defexception [:message, :code]

    @impl true
    def exception(opts) do
      %__MODULE__{
        message: Keyword.get(opts, :message, "Service error"),
        code: Keyword.get(opts, :code)
      }
    end
  end

  # User struct
  defmodule User do
    @moduledoc "Represents a user in the system"

    @enforce_keys [:id, :name]
    defstruct [:id, :name, :email]

    @type t :: %__MODULE__{
      id: String.t(),
      name: String.t(),
      email: String.t() | nil
    }

    @doc "Create a new user"
    @spec new(String.t(), String.t(), String.t() | nil) :: t()
    def new(id, name, email \\ nil) do
      %__MODULE__{id: id, name: name, email: email}
    end

    @doc "Get display name for user"
    @spec get_display_name(t()) :: String.t()
    def get_display_name(%__MODULE__{name: name}) when name != "" and not is_nil(name), do: name
    def get_display_name(%__MODULE__{email: email}) when not is_nil(email), do: email
    def get_display_name(%__MODULE__{id: id}), do: id

    @doc "Set user email"
    @spec set_email(t(), String.t()) :: t()
    def set_email(user, email), do: %{user | email: email}
  end

  # Behaviour definition
  defmodule ServiceBehaviour do
    @moduledoc "Behaviour for services"

    @callback initialize(state :: any()) :: {:ok, any()} | {:error, term()}
    @callback is_ready?(state :: any()) :: boolean()
  end

  # UserService using GenServer
  defmodule UserService do
    @moduledoc "Service for managing users"

    use GenServer
    @behaviour Sample.ServiceBehaviour

    alias Sample.User

    # Client API

    @doc "Start the UserService"
    def start_link(opts \\ []) do
      GenServer.start_link(__MODULE__, opts, name: __MODULE__)
    end

    @doc "Initialize the service"
    @impl Sample.ServiceBehaviour
    def initialize(state), do: {:ok, %{state | initialized: true}}

    @doc "Check if service is ready"
    @impl Sample.ServiceBehaviour
    def is_ready?(state), do: state.initialized

    @doc "Create a new user"
    @spec create_user(String.t(), String.t(), String.t() | nil) :: {:ok, User.t()} | {:error, term()}
    def create_user(id, name, email \\ nil) do
      GenServer.call(__MODULE__, {:create_user, id, name, email})
    end

    @doc "Get a user by ID"
    @spec get_user(String.t()) :: {:ok, User.t()} | {:error, :not_found}
    def get_user(id) do
      GenServer.call(__MODULE__, {:get_user, id})
    end

    @doc "Delete a user by ID"
    @spec delete_user(String.t()) :: :ok | {:error, :not_found}
    def delete_user(id) do
      GenServer.call(__MODULE__, {:delete_user, id})
    end

    # Server callbacks

    @impl GenServer
    def init(_opts) do
      {:ok, %{users: %{}, initialized: false}}
    end

    @impl GenServer
    def handle_call({:create_user, id, name, email}, _from, state) do
      if id == "" or is_nil(id) do
        {:reply, {:error, :invalid_id}, state}
      else
        user = User.new(id, name, email)
        new_state = put_in(state, [:users, id], user)
        on_user_created(user)
        {:reply, {:ok, user}, new_state}
      end
    end

    @impl GenServer
    def handle_call({:get_user, id}, _from, state) do
      case Map.get(state.users, id) do
        nil -> {:reply, {:error, :not_found}, state}
        user -> {:reply, {:ok, user}, state}
      end
    end

    @impl GenServer
    def handle_call({:delete_user, id}, _from, state) do
      if Map.has_key?(state.users, id) do
        new_state = %{state | users: Map.delete(state.users, id)}
        {:reply, :ok, new_state}
      else
        {:reply, {:error, :not_found}, state}
      end
    end

    # Private functions

    defp on_user_created(_user) do
      # Internal callback
      :ok
    end
  end

  # Public module functions

  @doc """
  Calculate factorial of n.

  ## Examples

      iex> Sample.calculate_factorial(5)
      120

  """
  @spec calculate_factorial(non_neg_integer()) :: non_neg_integer()
  def calculate_factorial(n) when n <= 1, do: 1
  def calculate_factorial(n), do: n * calculate_factorial(n - 1)

  @doc """
  Calculate nth Fibonacci number.
  """
  @spec fibonacci(non_neg_integer()) :: non_neg_integer()
  def fibonacci(0), do: 0
  def fibonacci(1), do: 1
  def fibonacci(n), do: fibonacci(n - 1) + fibonacci(n - 2)

  @doc """
  Fetch data from URL asynchronously.
  """
  @spec fetch_data(String.t(), number()) :: {:ok, map()} | {:error, term()}
  def fetch_data(url, timeout \\ @default_timeout) do
    # Simulated async fetch
    Task.async(fn ->
      %{"url" => url, "status" => "ok", "timeout" => timeout}
    end)
    |> Task.await()
    |> then(&{:ok, &1})
  end

  @doc """
  Process items with optional validation.
  """
  @spec process_items([String.t()], boolean()) :: [String.t()]
  def process_items(items, validate \\ true) do
    items
    |> maybe_filter(validate)
    |> Enum.map(&String.trim/1)
    |> Enum.map(&String.downcase/1)
  end

  defp maybe_filter(items, true), do: Enum.filter(items, &(&1 != ""))
  defp maybe_filter(items, false), do: items

  @doc """
  Execute function with retry.
  """
  @spec with_retry((() -> any()), non_neg_integer()) :: {:ok, any()} | {:error, term()}
  def with_retry(func, max_retries \\ @max_retries) do
    do_retry(func, max_retries, nil)
  end

  defp do_retry(_func, 0, last_error), do: {:error, last_error || :max_retries_exceeded}
  defp do_retry(func, retries, _last_error) do
    try do
      {:ok, func.()}
    rescue
      e -> do_retry(func, retries - 1, e)
    end
  end

  # Guards example
  @doc "Check if value is positive integer"
  defguard is_positive(value) when is_integer(value) and value > 0

  @doc "Double a positive value"
  @spec double_positive(integer()) :: integer()
  def double_positive(n) when is_positive(n), do: n * 2

  # Protocol implementation
  defimpl String.Chars, for: User do
    def to_string(user), do: "User(#{user.id}, #{user.name})"
  end

  # Sigil example
  def sigil_example do
    ~w(apple banana cherry)
  end

  # Macro example (simple)
  defmacro debug(expr) do
    quote do
      IO.inspect(unquote(expr), label: unquote(Macro.to_string(expr)))
    end
  end
end

# Main entry point
defmodule Sample.Main do
  @moduledoc false

  alias Sample.{User, UserService}

  def main do
    {:ok, _pid} = UserService.start_link()

    case UserService.create_user("1", "Test User", "test@example.com") do
      {:ok, user} ->
        IO.puts("Created user: #{User.get_display_name(user)}")

      {:error, reason} ->
        IO.puts("Error: #{inspect(reason)}")
    end

    result = Sample.calculate_factorial(5)
    IO.puts("Factorial: #{result}")
  end
end
