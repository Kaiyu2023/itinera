resource "aws_cloudwatch_log_group" "api" {
  name                        = "/aws/lambda/${local.function_name}"
  retention_in_days           = var.log_retention_days
  deletion_protection_enabled = var.log_deletion_protection_enabled

  tags = merge(local.common_tags, {
    Name = "/aws/lambda/${local.function_name}"
  })
}

resource "aws_lambda_function" "api" {
  function_name = local.function_name
  description   = "Itinera HTTP API"
  filename      = var.lambda_package_path
  role          = aws_iam_role.api.arn

  source_code_hash = var.lambda_source_code_hash
  runtime          = "provided.al2023"
  handler          = "bootstrap"
  architectures    = [var.lambda_architecture]
  memory_size      = var.lambda_memory_size
  timeout          = var.lambda_timeout_seconds

  reserved_concurrent_executions = var.lambda_reserved_concurrency
  publish                        = true

  environment {
    variables = {
      ITINERA_CF_ACCESS_TEAM_DOMAIN = local.cloudflare_access_team_domain
      ITINERA_CF_ACCESS_AUDIENCE    = trimspace(var.cloudflare_access_audience)
      ITINERA_DYNAMODB_TABLE        = aws_dynamodb_table.data.name
      # Hashes are password verifiers, not replayable secrets. The plaintext
      # exists only as a Cloudflare Worker secret and in the rotation workflow.
      ITINERA_ORIGIN_SECRET_SHA256_HASHES = join(",", var.origin_secret_sha256_hashes)
    }
  }

  tracing_config {
    mode = "PassThrough"
  }

  tags = merge(local.common_tags, {
    Name = local.function_name
  })

  depends_on = [
    aws_cloudwatch_log_group.api,
    aws_iam_role_policy.runtime,
  ]
}

resource "aws_lambda_alias" "live" {
  name             = "live"
  description      = "Currently deployed Itinera API version"
  function_name    = aws_lambda_function.api.function_name
  function_version = aws_lambda_function.api.version
}

# Cloudflare proxies to this URL and the application independently validates
# every Cloudflare Access assertion. With NONE auth, AWS Provider 6.x installs
# both required public resource-policy statements, including the condition that
# limits lambda:InvokeFunction to calls made through the Function URL.
resource "aws_lambda_function_url" "api" {
  function_name      = aws_lambda_function.api.function_name
  qualifier          = aws_lambda_alias.live.name
  authorization_type = "NONE"
  invoke_mode        = "BUFFERED"
}
