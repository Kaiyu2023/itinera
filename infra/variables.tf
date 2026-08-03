variable "name_prefix" {
  description = "Short, environment-specific prefix used for AWS resource names."
  type        = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{2,39}$", var.name_prefix))
    error_message = "name_prefix must be 3-40 lowercase letters, numbers, or hyphens, starting with a letter."
  }
}

variable "lambda_package_path" {
  description = "Path to the cargo-lambda zip artifact built by the private deployment workflow."
  type        = string

  validation {
    condition     = length(trimspace(var.lambda_package_path)) > 4 && endswith(lower(var.lambda_package_path), ".zip")
    error_message = "lambda_package_path must point to a .zip deployment package."
  }
}

variable "lambda_source_code_hash" {
  description = "Base64-encoded SHA-256 digest of lambda_package_path, normally filebase64sha256(path)."
  type        = string

  validation {
    condition     = can(regex("^[A-Za-z0-9+/]{43}=$", var.lambda_source_code_hash))
    error_message = "lambda_source_code_hash must be a base64-encoded SHA-256 digest."
  }
}

variable "cloudflare_access_team_domain" {
  description = "Cloudflare Access HTTPS team origin used as the JWT issuer."
  type        = string
  sensitive   = true

  validation {
    condition = can(regex(
      "^https://[A-Za-z0-9][A-Za-z0-9-]*\\.cloudflareaccess\\.com/?$",
      trimspace(var.cloudflare_access_team_domain),
    ))
    error_message = "cloudflare_access_team_domain must be an HTTPS origin such as https://your-team.cloudflareaccess.com."
  }
}

variable "cloudflare_access_audience" {
  description = "Cloudflare Access application audience tag checked by the API."
  type        = string
  sensitive   = true

  validation {
    condition = (
      length(trimspace(var.cloudflare_access_audience)) >= 1 &&
      length(trimspace(var.cloudflare_access_audience)) <= 1024
    )
    error_message = "cloudflare_access_audience must contain 1-1024 characters."
  }
}

variable "lambda_architecture" {
  description = "Instruction-set architecture of both the deployment package and Lambda function."
  type        = string
  default     = "arm64"

  validation {
    condition     = contains(["arm64", "x86_64"], var.lambda_architecture)
    error_message = "lambda_architecture must be arm64 or x86_64."
  }
}

variable "lambda_memory_size" {
  description = "Lambda memory allocation in MiB."
  type        = number
  default     = 512

  validation {
    condition = (
      var.lambda_memory_size == floor(var.lambda_memory_size) &&
      var.lambda_memory_size >= 128 &&
      var.lambda_memory_size <= 10240
    )
    error_message = "lambda_memory_size must be an integer between 128 and 10240 MiB."
  }
}

variable "lambda_timeout_seconds" {
  description = "Maximum Lambda request duration in seconds."
  type        = number
  default     = 15

  validation {
    condition = (
      var.lambda_timeout_seconds == floor(var.lambda_timeout_seconds) &&
      var.lambda_timeout_seconds >= 1 &&
      var.lambda_timeout_seconds <= 900
    )
    error_message = "lambda_timeout_seconds must be an integer between 1 and 900."
  }
}

variable "lambda_reserved_concurrency" {
  description = "Hard concurrency ceiling for the public Function URL."
  type        = number
  default     = 10

  validation {
    condition = (
      var.lambda_reserved_concurrency == floor(var.lambda_reserved_concurrency) &&
      var.lambda_reserved_concurrency >= 1 &&
      var.lambda_reserved_concurrency <= 100
    )
    error_message = "lambda_reserved_concurrency must be an integer between 1 and 100."
  }
}

variable "table_read_capacity" {
  description = "Provisioned read capacity units for the base table."
  type        = number
  default     = 10

  validation {
    condition = (
      var.table_read_capacity == floor(var.table_read_capacity) &&
      var.table_read_capacity >= 1 &&
      var.table_read_capacity <= 40000
    )
    error_message = "table_read_capacity must be an integer between 1 and 40000."
  }
}

variable "table_write_capacity" {
  description = "Provisioned write capacity units for the base table."
  type        = number
  default     = 5

  validation {
    condition = (
      var.table_write_capacity == floor(var.table_write_capacity) &&
      var.table_write_capacity >= 1 &&
      var.table_write_capacity <= 40000
    )
    error_message = "table_write_capacity must be an integer between 1 and 40000."
  }
}

variable "gsi_read_capacity" {
  description = "Provisioned read capacity units for gsi1."
  type        = number
  default     = 5

  validation {
    condition = (
      var.gsi_read_capacity == floor(var.gsi_read_capacity) &&
      var.gsi_read_capacity >= 1 &&
      var.gsi_read_capacity <= 40000
    )
    error_message = "gsi_read_capacity must be an integer between 1 and 40000."
  }
}

variable "gsi_write_capacity" {
  description = "Provisioned write capacity units for gsi1."
  type        = number
  default     = 5

  validation {
    condition = (
      var.gsi_write_capacity == floor(var.gsi_write_capacity) &&
      var.gsi_write_capacity >= 1 &&
      var.gsi_write_capacity <= 40000
    )
    error_message = "gsi_write_capacity must be an integer between 1 and 40000."
  }
}

variable "enforce_free_tier_capacity_limit" {
  description = "Reject combined table and GSI capacity above the DynamoDB 25 RCU/WCU account allowance."
  type        = bool
  default     = true
}

variable "dynamodb_deletion_protection_enabled" {
  description = "Prevent accidental deletion of the application table."
  type        = bool
  default     = true
}

variable "dynamodb_point_in_time_recovery_days" {
  description = "Number of days retained by DynamoDB point-in-time recovery."
  type        = number
  default     = 5

  validation {
    condition = (
      var.dynamodb_point_in_time_recovery_days == floor(var.dynamodb_point_in_time_recovery_days) &&
      var.dynamodb_point_in_time_recovery_days >= 1 &&
      var.dynamodb_point_in_time_recovery_days <= 35
    )
    error_message = "dynamodb_point_in_time_recovery_days must be an integer between 1 and 35."
  }
}

variable "log_retention_days" {
  description = "CloudWatch application-log retention period."
  type        = number
  default     = 30

  validation {
    condition = contains([
      1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731,
      1096, 1827, 2192, 2557, 2922, 3288, 3653,
    ], var.log_retention_days)
    error_message = "log_retention_days must be a CloudWatch Logs supported retention period."
  }
}

variable "log_deletion_protection_enabled" {
  description = "Prevent accidental deletion of the Lambda log group. Disable explicitly before an intentional teardown."
  type        = bool
  default     = true
}

variable "alarm_action_arns" {
  description = "SNS topic ARNs notified when an application alarm enters ALARM state. Leave empty to create no alarms."
  type        = list(string)
  default     = []

  validation {
    condition = (
      length(var.alarm_action_arns) <= 5 &&
      alltrue([
        for arn in var.alarm_action_arns : can(regex("^arn:aws[a-zA-Z-]*:sns:[^:]+:[0-9]{12}:[^:]+$", arn))
      ])
    )
    error_message = "alarm_action_arns must contain at most five SNS topic ARNs."
  }
}

variable "permissions_boundary_arn" {
  description = "Optional IAM permissions-boundary policy ARN for the Lambda execution role."
  type        = string
  default     = null
  nullable    = true

  validation {
    condition = var.permissions_boundary_arn == null ? true : can(regex(
      "^arn:aws[a-zA-Z-]*:iam::[0-9]{12}:policy/.+$",
      var.permissions_boundary_arn,
    ))
    error_message = "permissions_boundary_arn must be null or an IAM policy ARN."
  }
}

variable "tags" {
  description = "Additional tags applied to every taggable resource."
  type        = map(string)
  default     = {}
}
