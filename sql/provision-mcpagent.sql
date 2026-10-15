-- Read-only MariaDB account for the MCP agent's Tapo application.
-- Run as an administrator through the local Unix socket.
-- Supply @mcpagent_password in the same client session before this file.
-- config/provision-mcpagent.sql is the protected, ready-to-run local copy.
-- Refuses an existing account; never changes the collector or database schema.
-- Account DDL is not transactional. Stop on errors; do not use --force.

DELIMITER //
BEGIN NOT ATOMIC
    IF @mcpagent_password IS NULL OR CHAR_LENGTH(@mcpagent_password) < 16 THEN
        SIGNAL SQLSTATE '45000'
            SET MESSAGE_TEXT = 'Supply @mcpagent_password with at least 16 characters';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = 'tapoagent'
    ) THEN
        SIGNAL SQLSTATE '45000'
            SET MESSAGE_TEXT = 'Database tapoagent does not exist; provision it first';
    END IF;
    IF (
        SELECT COUNT(*) FROM information_schema.TABLES
        WHERE TABLE_SCHEMA = 'tapoagent' AND TABLE_TYPE = 'BASE TABLE'
          AND TABLE_NAME IN ('sensors', 'measurements', 'collector_status', 'schema_migrations')
    ) <> 4 THEN
        SIGNAL SQLSTATE '45000'
            SET MESSAGE_TEXT = 'Required tapoagent tables are missing; refusing provisioning';
    END IF;
    IF EXISTS (SELECT 1 FROM mysql.user WHERE User = 'mcpagent') THEN
        SIGNAL SQLSTATE '45000'
            SET MESSAGE_TEXT = 'User mcpagent already exists; refusing provisioning';
    END IF;
END//
DELIMITER ;

-- Hash before constructing SQL, so quotes/backslashes and SQL mode cannot
-- change the password literal. Use the native authentication format explicitly.
SET SESSION old_passwords = 0;
SET @create_mcpagent = CONCAT(
    'CREATE USER ''mcpagent''@''localhost'' IDENTIFIED BY PASSWORD ',
    QUOTE(PASSWORD(@mcpagent_password))
);
SET @mcpagent_password = NULL;
PREPARE create_mcpagent FROM @create_mcpagent;
EXECUTE create_mcpagent;
DEALLOCATE PREPARE create_mcpagent;
SET @create_mcpagent = NULL;

GRANT SELECT ON `tapoagent`.`sensors` TO 'mcpagent'@'localhost';
GRANT SELECT ON `tapoagent`.`measurements` TO 'mcpagent'@'localhost';
GRANT SELECT ON `tapoagent`.`collector_status` TO 'mcpagent'@'localhost';
GRANT SELECT ON `tapoagent`.`schema_migrations` TO 'mcpagent'@'localhost';

-- Report grants without exposing password hashes or authentication material.
SELECT TABLE_NAME, PRIVILEGE_TYPE
FROM information_schema.TABLE_PRIVILEGES
WHERE GRANTEE = '''mcpagent''@''localhost''' AND TABLE_SCHEMA = 'tapoagent'
ORDER BY TABLE_NAME, PRIVILEGE_TYPE;
