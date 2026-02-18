{- |
Module      : Sample
Description : Sample Haskell file for testing code chunking
Copyright   : (c) ProximaDB Team, 2025
License     : Apache-2.0

This module contains various Haskell constructs to test AST parsing.
-}

module Sample
    ( -- * Constants
      maxRetries
    , defaultTimeout
      -- * Data Types
    , User(..)
    , ServiceError(..)
    , ServiceStatus(..)
      -- * Type Classes
    , Service(..)
    , Displayable(..)
      -- * User Operations
    , createUser
    , getDisplayName
    , setEmail
      -- * UserService
    , UserService
    , newUserService
    , addUser
    , getUser
    , deleteUser
      -- * Utility Functions
    , calculateFactorial
    , fibonacci
    , processItems
    , withRetry
    ) where

import qualified Data.Map.Strict as Map
import Data.Map.Strict (Map)
import Control.Monad (when)
import Control.Exception (Exception, throw, catch, SomeException)
import Data.Maybe (fromMaybe)

-- | Maximum number of retries for operations
maxRetries :: Int
maxRetries = 3

-- | Default timeout in seconds
defaultTimeout :: Double
defaultTimeout = 30.0

-- | Represents a user in the system
data User = User
    { userId    :: !String
    , userName  :: !String
    , userEmail :: !(Maybe String)
    } deriving (Show, Eq)

-- | Custom exception for service errors
data ServiceError
    = NotFoundError String
    | InvalidInputError String
    | InternalError String
    deriving (Show, Eq)

instance Exception ServiceError

-- | Service status enumeration
data ServiceStatus
    = Pending
    | Running
    | Stopped
    | Error String
    deriving (Show, Eq)

-- | Type class for displayable objects
class Displayable a where
    -- | Get display representation
    display :: a -> String

instance Displayable User where
    display = getDisplayName

-- | Type class for services
class Service s where
    -- | Initialize the service
    initialize :: s -> IO s

    -- | Check if service is ready
    isReady :: s -> Bool

-- | Create a new user
createUser :: String -> String -> Maybe String -> Either ServiceError User
createUser uid name email
    | null uid  = Left $ InvalidInputError "ID cannot be empty"
    | otherwise = Right $ User uid name email

-- | Get display name for a user
getDisplayName :: User -> String
getDisplayName user =
    case userName user of
        "" -> fromMaybe (userId user) (userEmail user)
        n  -> n

-- | Set user email
setEmail :: String -> User -> User
setEmail email user = user { userEmail = Just email }

-- | User service data type
data UserService = UserService
    { users       :: !(Map String User)
    , initialized :: !Bool
    } deriving (Show)

-- | Create a new UserService
newUserService :: UserService
newUserService = UserService Map.empty False

instance Service UserService where
    initialize svc = return $ svc { initialized = True }
    isReady = initialized

-- | Add a user to the service
addUser :: String -> String -> Maybe String -> UserService -> Either ServiceError UserService
addUser uid name email svc
    | null uid = Left $ InvalidInputError "ID cannot be empty"
    | otherwise =
        let user = User uid name email
            newUsers = Map.insert uid user (users svc)
        in Right $ svc { users = newUsers }

-- | Get a user by ID
getUser :: String -> UserService -> Maybe User
getUser uid svc = Map.lookup uid (users svc)

-- | Delete a user by ID
deleteUser :: String -> UserService -> (Bool, UserService)
deleteUser uid svc =
    let exists = Map.member uid (users svc)
        newUsers = Map.delete uid (users svc)
    in (exists, svc { users = newUsers })

-- | Calculate factorial of n
calculateFactorial :: Integer -> Integer
calculateFactorial n
    | n <= 1    = 1
    | otherwise = n * calculateFactorial (n - 1)

-- | Calculate nth Fibonacci number
fibonacci :: Int -> Integer
fibonacci n = fibs !! n
  where
    fibs = 0 : 1 : zipWith (+) fibs (tail fibs)

-- | Process items with optional validation
processItems :: Bool -> [String] -> [String]
processItems validate items =
    let filtered = if validate
                   then filter (not . null) items
                   else items
    in map (map toLowercase . trim) filtered
  where
    toLowercase c
        | c >= 'A' && c <= 'Z' = toEnum (fromEnum c + 32)
        | otherwise = c
    trim = dropWhile (== ' ') . reverse . dropWhile (== ' ') . reverse

-- | Execute an action with retry
withRetry :: Int -> IO a -> IO (Either SomeException a)
withRetry maxTries action = go maxTries
  where
    go 0 = return $ Left $ toException $ InternalError "Max retries exceeded"
    go n = catch (Right <$> action) handler
      where
        handler :: SomeException -> IO (Either SomeException a)
        handler e = go (n - 1)

    toException :: Exception e => e -> SomeException
    toException = toException

-- | Higher-order function example
mapUsers :: (User -> a) -> UserService -> [a]
mapUsers f svc = map f $ Map.elems (users svc)

-- | Generic container
data Container a = Container
    { containerValue :: a
    } deriving (Show, Eq, Functor)

-- | Type alias
type UserId = String
type UserMap = Map UserId User

-- | Newtype wrapper
newtype Email = Email { unEmail :: String }
    deriving (Show, Eq)

-- | Pattern synonym
pattern AdminUser :: String -> User
pattern AdminUser name = User "admin" name Nothing

-- | Main entry point
main :: IO ()
main = do
    let svc = newUserService
    svc' <- initialize svc

    case addUser "1" "Test User" (Just "test@example.com") svc' of
        Left err -> putStrLn $ "Error: " ++ show err
        Right svc'' -> do
            case getUser "1" svc'' of
                Nothing -> putStrLn "User not found"
                Just user -> putStrLn $ "Created user: " ++ getDisplayName user

    let result = calculateFactorial 5
    putStrLn $ "Factorial: " ++ show result
