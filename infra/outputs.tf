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
  description = "IAM-protected Lambda origin URL for negative smoke tests; never configure the Worker to call it."
  value       = aws_lambda_function_url.api.function_url
  sensitive   = true
}

output "cloudfront_distribution_id" {
  description = "ID of the API CloudFront distribution."
  value       = aws_cloudfront_distribution.api.id
}

output "cloudfront_distribution_arn" {
  description = "ARN used to scope the Lambda Function URL resource policy."
  value       = aws_cloudfront_distribution.api.arn
}

output "cloudfront_domain_name" {
  description = "CloudFront hostname called only by the Access-protected Worker."
  value       = aws_cloudfront_distribution.api.domain_name
}

output "cloudflare_worker_origin_url" {
  description = "Validated ITINERA_CLOUDFRONT_URL binding for the private Cloudflare Worker deployment."
  value       = "https://${aws_cloudfront_distribution.api.domain_name}/"
}
