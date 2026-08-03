output "dynamodb_table_name" {
  description = "Name passed to ITINERA_DYNAMODB_TABLE."
  value       = aws_dynamodb_table.data.name
}

output "dynamodb_table_arn" {
  description = "ARN of the application table."
  value       = aws_dynamodb_table.data.arn
}

output "dynamodb_gsi_name" {
  description = "Name of the sparse general-purpose secondary index."
  value       = local.gsi_name
}

output "dynamodb_gsi_arn" {
  description = "ARN of gsi1."
  value       = local.dynamodb_index_arn
}

output "lambda_function_name" {
  description = "Name of the API Lambda function."
  value       = aws_lambda_function.api.function_name
}

output "lambda_function_arn" {
  description = "Unqualified ARN of the API Lambda function."
  value       = aws_lambda_function.api.arn
}

output "lambda_live_alias_arn" {
  description = "ARN of the live API alias targeted by the Function URL."
  value       = aws_lambda_alias.live.arn
}

output "lambda_execution_role_arn" {
  description = "ARN of the least-privilege API runtime role."
  value       = aws_iam_role.api.arn
}

output "lambda_function_url" {
  description = "Origin URL for the private deployment repository to configure behind Cloudflare."
  value       = aws_lambda_function_url.api.function_url
  sensitive   = true
}
