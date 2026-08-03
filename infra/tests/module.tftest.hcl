mock_provider "aws" {
  override_during = plan

  mock_resource "aws_dynamodb_table" {
    defaults = {
      arn = "arn:aws:dynamodb:eu-west-2:123456789012:table/itinera-test-data"
    }
  }

  mock_resource "aws_cloudwatch_log_group" {
    defaults = {
      arn = "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/lambda/itinera-test-api"
    }
  }

  mock_resource "aws_lambda_function" {
    defaults = {
      arn     = "arn:aws:lambda:eu-west-2:123456789012:function:itinera-test-api"
      version = "1"
    }
  }

  mock_resource "aws_lambda_alias" {
    defaults = {
      arn = "arn:aws:lambda:eu-west-2:123456789012:function:itinera-test-api:live"
    }
  }

  mock_resource "aws_lambda_function_url" {
    defaults = {
      function_url = "https://example.lambda-url.eu-west-2.on.aws/"
    }
  }
}

variables {
  name_prefix                   = "itinera-test"
  lambda_package_path           = "dist/bootstrap.zip"
  lambda_source_code_hash       = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
  cloudflare_access_team_domain = "https://example.cloudflareaccess.com/"
  cloudflare_access_audience    = "test-audience"
}

run "secure_cost_conscious_defaults" {
  command = plan

  assert {
    condition     = aws_dynamodb_table.data.billing_mode == "PROVISIONED" && aws_dynamodb_table.data.table_class == "STANDARD"
    error_message = "The table must use provisioned Standard capacity."
  }

  assert {
    condition     = aws_dynamodb_table.data.deletion_protection_enabled && aws_dynamodb_table.data.point_in_time_recovery[0].enabled
    error_message = "The table must protect against deletion and support point-in-time recovery."
  }

  assert {
    condition     = aws_dynamodb_table.data.hash_key == "pk" && aws_dynamodb_table.data.range_key == "sk"
    error_message = "The base table keys must match the documented single-table design."
  }

  assert {
    condition     = one(aws_dynamodb_table.data.global_secondary_index).name == "gsi1"
    error_message = "The table must expose the documented gsi1 index."
  }

  assert {
    condition = (
      aws_dynamodb_table.data.read_capacity + one(aws_dynamodb_table.data.global_secondary_index).read_capacity <= 25 &&
      aws_dynamodb_table.data.write_capacity + one(aws_dynamodb_table.data.global_secondary_index).write_capacity <= 25
    )
    error_message = "Default table and index capacity must remain within the account-wide free-tier allowance."
  }

  assert {
    condition = (
      aws_lambda_function.api.runtime == "provided.al2023" &&
      aws_lambda_function.api.architectures == tolist(["arm64"]) &&
      aws_lambda_function.api.reserved_concurrent_executions == 10
    )
    error_message = "The Lambda must use the expected Rust runtime, architecture, and concurrency ceiling."
  }

  assert {
    condition = (
      aws_lambda_function.api.environment[0].variables["ITINERA_DYNAMODB_TABLE"] == aws_dynamodb_table.data.name &&
      aws_lambda_function.api.environment[0].variables["ITINERA_CF_ACCESS_TEAM_DOMAIN"] == "https://example.cloudflareaccess.com" &&
      !contains(keys(aws_lambda_function.api.environment[0].variables), "ITINERA_DEV_AUTH_ENABLED")
    )
    error_message = "Production runtime configuration is incomplete or includes development authentication."
  }

  assert {
    condition = (
      toset(one([
        for statement in jsondecode(aws_iam_role_policy.runtime.policy).Statement : statement.Action
        if statement.Sid == "ReadAndWriteApplicationTable"
        ])) == toset([
        "dynamodb:BatchGetItem",
        "dynamodb:DeleteItem",
        "dynamodb:GetItem",
        "dynamodb:PutItem",
        "dynamodb:TransactGetItems",
        "dynamodb:TransactWriteItems",
        "dynamodb:UpdateItem",
      ]) &&
      one([
        for statement in jsondecode(aws_iam_role_policy.runtime.policy).Statement : statement.Resource
        if statement.Sid == "ReadAndWriteApplicationTable"
      ]) == aws_dynamodb_table.data.arn &&
      one([
        for statement in jsondecode(aws_iam_role_policy.runtime.policy).Statement : statement.Action
        if statement.Sid == "QueryApplicationTableAndIndex"
      ]) == "dynamodb:Query" &&
      toset(one([
        for statement in jsondecode(aws_iam_role_policy.runtime.policy).Statement : statement.Resource
        if statement.Sid == "QueryApplicationTableAndIndex"
      ])) == toset([aws_dynamodb_table.data.arn, "${aws_dynamodb_table.data.arn}/index/gsi1"]) &&
      !strcontains(aws_iam_role_policy.runtime.policy, "dynamodb:Scan") &&
      !strcontains(aws_iam_role_policy.runtime.policy, "dynamodb:*")
    )
    error_message = "Runtime DynamoDB IAM must contain only the documented data operations on the exact table and index."
  }

  assert {
    condition = (
      toset(one([
        for statement in jsondecode(aws_iam_role_policy.runtime.policy).Statement : statement.Action
        if statement.Sid == "WriteApplicationLogs"
      ])) == toset(["logs:CreateLogStream", "logs:PutLogEvents"]) &&
      one([
        for statement in jsondecode(aws_iam_role_policy.runtime.policy).Statement : statement.Resource
        if statement.Sid == "WriteApplicationLogs"
      ]) == "${aws_cloudwatch_log_group.api.arn}:*"
    )
    error_message = "Runtime logging IAM must write only to the pre-created application log group."
  }

  assert {
    condition = (
      aws_lambda_function_url.api.authorization_type == "NONE" &&
      aws_lambda_function_url.api.qualifier == aws_lambda_alias.live.name
    )
    error_message = "Cloudflare must reach a stable live-alias Function URL."
  }

  assert {
    condition = (
      length(aws_cloudwatch_metric_alarm.lambda_failure) == 0 &&
      length(aws_cloudwatch_metric_alarm.function_url_server_errors) == 0 &&
      length(aws_cloudwatch_metric_alarm.lambda_concurrency) == 0 &&
      length(aws_cloudwatch_metric_alarm.dynamodb_throttle) == 0
    )
    error_message = "Omitting SNS destinations must omit otherwise unactionable alarms."
  }
}

run "creates_actionable_alarms_when_sns_is_configured" {
  command = plan

  variables {
    alarm_action_arns = ["arn:aws:sns:eu-west-2:123456789012:itinera-test-operations"]
  }

  assert {
    condition = (
      length(aws_cloudwatch_metric_alarm.lambda_failure) == 2 &&
      length(aws_cloudwatch_metric_alarm.function_url_server_errors) == 1 &&
      length(aws_cloudwatch_metric_alarm.lambda_concurrency) == 1 &&
      length(aws_cloudwatch_metric_alarm.dynamodb_throttle) == 4 &&
      alltrue([
        for alarm in aws_cloudwatch_metric_alarm.lambda_failure :
        toset(alarm.alarm_actions) == toset(["arn:aws:sns:eu-west-2:123456789012:itinera-test-operations"])
      ])
    )
    error_message = "Supplying SNS destinations must create all actionable alarms."
  }
}

run "rejects_capacity_above_free_tier_limit" {
  command = plan

  variables {
    table_read_capacity = 21
    gsi_read_capacity   = 5
  }

  expect_failures = [aws_dynamodb_table.data]
}

run "allows_deliberate_capacity_override" {
  command = plan

  variables {
    table_read_capacity              = 21
    gsi_read_capacity                = 5
    enforce_free_tier_capacity_limit = false
  }

  assert {
    condition     = aws_dynamodb_table.data.read_capacity + one(aws_dynamodb_table.data.global_secondary_index).read_capacity == 26
    error_message = "An explicit paid-capacity override should be honored."
  }
}

run "rejects_non_cloudflare_issuer" {
  command = plan

  variables {
    cloudflare_access_team_domain = "https://example.com"
  }

  expect_failures = [var.cloudflare_access_team_domain]
}
