locals {
  dynamodb_index_arn = "${aws_dynamodb_table.data.arn}/index/${local.gsi_name}"

  lambda_assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid    = "LambdaAssumeRole"
      Effect = "Allow"
      Action = "sts:AssumeRole"
      Principal = {
        Service = "lambda.amazonaws.com"
      }
    }]
  })

  lambda_runtime_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "WriteApplicationLogs"
        Effect = "Allow"
        Action = [
          "logs:CreateLogStream",
          "logs:PutLogEvents",
        ]
        Resource = "${aws_cloudwatch_log_group.api.arn}:*"
      },
      {
        Sid    = "ReadAndWriteApplicationTable"
        Effect = "Allow"
        Action = [
          "dynamodb:BatchGetItem",
          "dynamodb:DeleteItem",
          "dynamodb:GetItem",
          "dynamodb:PutItem",
          "dynamodb:TransactGetItems",
          "dynamodb:TransactWriteItems",
          "dynamodb:UpdateItem",
        ]
        Resource = aws_dynamodb_table.data.arn
      },
      {
        Sid      = "QueryApplicationTableAndIndex"
        Effect   = "Allow"
        Action   = "dynamodb:Query"
        Resource = [aws_dynamodb_table.data.arn, local.dynamodb_index_arn]
      },
    ]
  })
}

resource "aws_iam_role" "api" {
  name                 = "${var.name_prefix}-api-execution"
  description          = "Runtime identity for the Itinera API Lambda"
  assume_role_policy   = local.lambda_assume_role_policy
  permissions_boundary = var.permissions_boundary_arn

  tags = merge(local.common_tags, {
    Name = "${var.name_prefix}-api-execution"
  })
}

resource "aws_iam_role_policy" "runtime" {
  name   = "${var.name_prefix}-api-runtime"
  role   = aws_iam_role.api.id
  policy = local.lambda_runtime_policy
}
