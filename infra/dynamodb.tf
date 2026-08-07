resource "aws_dynamodb_table" "data" {
  name           = local.table_name
  billing_mode   = "PROVISIONED"
  table_class    = "STANDARD"
  hash_key       = "pk"
  range_key      = "sk"
  read_capacity  = var.table_read_capacity
  write_capacity = var.table_write_capacity

  deletion_protection_enabled = var.dynamodb_deletion_protection_enabled

  # DynamoDB always encrypts at rest. Omitting server_side_encryption selects
  # the cost-conscious AWS-owned key documented for this small application;
  # setting enabled = true would instead select a KMS-managed key.

  attribute {
    name = "pk"
    type = "S"
  }

  attribute {
    name = "sk"
    type = "S"
  }

  attribute {
    name = "gsi1pk"
    type = "S"
  }

  attribute {
    name = "gsi1sk"
    type = "S"
  }

  global_secondary_index {
    name            = local.gsi_name
    projection_type = "ALL"
    read_capacity   = var.gsi_read_capacity
    write_capacity  = var.gsi_write_capacity

    key_schema {
      attribute_name = "gsi1pk"
      key_type       = "HASH"
    }

    key_schema {
      attribute_name = "gsi1sk"
      key_type       = "RANGE"
    }
  }

  point_in_time_recovery {
    enabled                 = true
    recovery_period_in_days = var.dynamodb_point_in_time_recovery_days
  }

  # Capability records still enforce expiry from application time. TTL only
  # reclaims bounded hourly service-usage and idempotency rows asynchronously.
  ttl {
    attribute_name = "ttl"
    enabled        = true
  }

  tags = merge(local.common_tags, {
    Name = local.table_name
  })

  lifecycle {
    precondition {
      condition = !var.enforce_free_tier_capacity_limit || (
        var.table_read_capacity + var.gsi_read_capacity <= 25 &&
        var.table_write_capacity + var.gsi_write_capacity <= 25
      )
      error_message = "Combined table and gsi1 capacity exceeds 25 RCU or WCU. Lower it or explicitly disable enforce_free_tier_capacity_limit after checking account-wide usage and budgets."
    }
  }
}
