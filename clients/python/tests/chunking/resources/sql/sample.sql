-- Sample SQL file for testing code chunking.
--
-- This file contains various SQL constructs to test AST parsing.

-- ============================================
-- Constants and Configuration
-- ============================================

-- Set configuration variables
SET @max_retries = 3;
SET @default_timeout = 30.0;

-- ============================================
-- Table Definitions
-- ============================================

-- Users table
CREATE TABLE IF NOT EXISTS users (
    id VARCHAR(255) PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_email (email),
    INDEX idx_created_at (created_at)
);

-- User roles table
CREATE TABLE IF NOT EXISTS user_roles (
    id INT AUTO_INCREMENT PRIMARY KEY,
    user_id VARCHAR(255) NOT NULL,
    role VARCHAR(50) NOT NULL,
    granted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    UNIQUE KEY unique_user_role (user_id, role)
);

-- Service configuration table
CREATE TABLE IF NOT EXISTS service_config (
    key_name VARCHAR(255) PRIMARY KEY,
    value_data TEXT,
    data_type VARCHAR(50) DEFAULT 'string',
    description TEXT,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
);

-- Audit log table
CREATE TABLE IF NOT EXISTS audit_log (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    entity_type VARCHAR(100) NOT NULL,
    entity_id VARCHAR(255) NOT NULL,
    action VARCHAR(50) NOT NULL,
    old_value JSON,
    new_value JSON,
    performed_by VARCHAR(255),
    performed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_entity (entity_type, entity_id),
    INDEX idx_performed_at (performed_at)
);

-- ============================================
-- Views
-- ============================================

-- View for users with their roles
CREATE OR REPLACE VIEW user_with_roles AS
SELECT
    u.id,
    u.name,
    u.email,
    GROUP_CONCAT(ur.role SEPARATOR ', ') AS roles,
    u.created_at
FROM users u
LEFT JOIN user_roles ur ON u.id = ur.user_id
GROUP BY u.id, u.name, u.email, u.created_at;

-- View for active users
CREATE OR REPLACE VIEW active_users AS
SELECT * FROM users
WHERE updated_at > DATE_SUB(CURRENT_TIMESTAMP, INTERVAL 30 DAY);

-- ============================================
-- Stored Procedures
-- ============================================

DELIMITER //

-- Procedure to create a new user
CREATE PROCEDURE create_user(
    IN p_id VARCHAR(255),
    IN p_name VARCHAR(255),
    IN p_email VARCHAR(255)
)
BEGIN
    DECLARE EXIT HANDLER FOR SQLEXCEPTION
    BEGIN
        ROLLBACK;
        RESIGNAL;
    END;

    START TRANSACTION;

    IF p_id IS NULL OR p_id = '' THEN
        SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'ID cannot be empty';
    END IF;

    INSERT INTO users (id, name, email)
    VALUES (p_id, p_name, p_email);

    -- Call internal callback
    CALL on_user_created(p_id);

    COMMIT;
END //

-- Procedure to get user by ID
CREATE PROCEDURE get_user(
    IN p_id VARCHAR(255)
)
BEGIN
    SELECT id, name, email, created_at, updated_at
    FROM users
    WHERE id = p_id;
END //

-- Procedure to delete user
CREATE PROCEDURE delete_user(
    IN p_id VARCHAR(255),
    OUT p_success BOOLEAN
)
BEGIN
    DELETE FROM users WHERE id = p_id;
    SET p_success = ROW_COUNT() > 0;
END //

-- Internal callback procedure
CREATE PROCEDURE on_user_created(
    IN p_user_id VARCHAR(255)
)
BEGIN
    -- Internal callback logic
    INSERT INTO audit_log (entity_type, entity_id, action, performed_by)
    VALUES ('user', p_user_id, 'created', 'system');
END //

-- Procedure to calculate factorial
CREATE PROCEDURE calculate_factorial(
    IN p_n INT,
    OUT p_result BIGINT
)
BEGIN
    DECLARE v_i INT DEFAULT 1;
    SET p_result = 1;

    WHILE v_i <= p_n DO
        SET p_result = p_result * v_i;
        SET v_i = v_i + 1;
    END WHILE;
END //

-- Procedure to process items
CREATE PROCEDURE process_items(
    IN p_items TEXT,
    IN p_validate BOOLEAN
)
BEGIN
    DECLARE v_item VARCHAR(255);
    DECLARE v_pos INT DEFAULT 1;
    DECLARE v_len INT;

    DROP TEMPORARY TABLE IF EXISTS temp_results;
    CREATE TEMPORARY TABLE temp_results (item VARCHAR(255));

    SET v_len = LENGTH(p_items);

    WHILE v_pos <= v_len DO
        SET v_item = SUBSTRING_INDEX(SUBSTRING(p_items, v_pos), ',', 1);
        SET v_pos = v_pos + LENGTH(v_item) + 1;
        SET v_item = LOWER(TRIM(v_item));

        IF NOT p_validate OR (v_item IS NOT NULL AND v_item != '') THEN
            INSERT INTO temp_results VALUES (v_item);
        END IF;
    END WHILE;

    SELECT * FROM temp_results;
END //

DELIMITER ;

-- ============================================
-- Functions
-- ============================================

DELIMITER //

-- Function to get display name
CREATE FUNCTION get_display_name(
    p_name VARCHAR(255),
    p_email VARCHAR(255),
    p_id VARCHAR(255)
) RETURNS VARCHAR(255)
DETERMINISTIC
BEGIN
    IF p_name IS NOT NULL AND p_name != '' THEN
        RETURN p_name;
    ELSEIF p_email IS NOT NULL THEN
        RETURN p_email;
    ELSE
        RETURN p_id;
    END IF;
END //

-- Function to calculate fibonacci
CREATE FUNCTION fibonacci(
    p_n INT
) RETURNS BIGINT
DETERMINISTIC
BEGIN
    DECLARE v_a BIGINT DEFAULT 0;
    DECLARE v_b BIGINT DEFAULT 1;
    DECLARE v_temp BIGINT;
    DECLARE v_i INT DEFAULT 2;

    IF p_n <= 0 THEN RETURN 0; END IF;
    IF p_n = 1 THEN RETURN 1; END IF;

    WHILE v_i <= p_n DO
        SET v_temp = v_a + v_b;
        SET v_a = v_b;
        SET v_b = v_temp;
        SET v_i = v_i + 1;
    END WHILE;

    RETURN v_b;
END //

DELIMITER ;

-- ============================================
-- Triggers
-- ============================================

DELIMITER //

-- Trigger for user updates
CREATE TRIGGER before_user_update
BEFORE UPDATE ON users
FOR EACH ROW
BEGIN
    SET NEW.updated_at = CURRENT_TIMESTAMP;
END //

-- Trigger to log user deletions
CREATE TRIGGER after_user_delete
AFTER DELETE ON users
FOR EACH ROW
BEGIN
    INSERT INTO audit_log (entity_type, entity_id, action, old_value, performed_by)
    VALUES ('user', OLD.id, 'deleted',
            JSON_OBJECT('name', OLD.name, 'email', OLD.email),
            'system');
END //

DELIMITER ;

-- ============================================
-- Sample Queries
-- ============================================

-- Insert sample users
INSERT INTO users (id, name, email) VALUES
    ('1', 'Test User', 'test@example.com'),
    ('2', 'Admin User', 'admin@example.com')
ON DUPLICATE KEY UPDATE name = VALUES(name);

-- Query with joins
SELECT
    u.id,
    u.name,
    get_display_name(u.name, u.email, u.id) AS display_name,
    COUNT(ur.role) AS role_count
FROM users u
LEFT JOIN user_roles ur ON u.id = ur.user_id
WHERE u.created_at > DATE_SUB(CURRENT_TIMESTAMP, INTERVAL 7 DAY)
GROUP BY u.id, u.name, u.email
HAVING role_count >= 0
ORDER BY u.created_at DESC
LIMIT 10;
