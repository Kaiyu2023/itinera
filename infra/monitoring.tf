locals {
  lambda_resource_dimension = "${aws_lambda_function.api.function_name}:${aws_lambda_alias.live.name}"

  lambda_failure_alarms = {
    errors = {
      metric_name = "Errors"
      description = "The Itinera API returned at least one Lambda runtime or function error."
    }
    throttles = {
      metric_name = "Throttles"
      description = "The Itinera API rejected at least one invocation at its concurrency limit."
    }
  }

  dynamodb_throttle_alarms = {
    table-read = {
      metric_name = "ReadThrottleEvents"
      dimensions = {
        TableName = aws_dynamodb_table.data.name
      }
    }
    table-write = {
      metric_name = "WriteThrottleEvents"
      dimensions = {
        TableName = aws_dynamodb_table.data.name
      }
    }
    gsi-read = {
      metric_name = "ReadThrottleEvents"
      dimensions = {
        TableName                = aws_dynamodb_table.data.name
        GlobalSecondaryIndexName = local.gsi_name
      }
    }
    gsi-write = {
      metric_name = "WriteThrottleEvents"
      dimensions = {
        TableName                = aws_dynamodb_table.data.name
        GlobalSecondaryIndexName = local.gsi_name
      }
    }
  }
}

resource "aws_cloudwatch_metric_alarm" "lambda_failure" {
  for_each = local.lambda_failure_alarms

  alarm_name          = "${var.name_prefix}-api-${each.key}"
  alarm_description   = each.value.description
  namespace           = "AWS/Lambda"
  metric_name         = each.value.metric_name
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  datapoints_to_alarm = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"

  dimensions = {
    FunctionName = aws_lambda_function.api.function_name
    Resource     = local.lambda_resource_dimension
  }

  alarm_actions             = var.alarm_action_arns
  insufficient_data_actions = []

  tags = local.common_tags
}
resource "aws_cloudwatch_metric_alarm" "function_url_server_errors" {
  alarm_name          = "${var.name_prefix}-api-url-5xx"
  alarm_description   = "The public Itinera Function URL returned at least one server error."
  namespace           = "AWS/Lambda"
  metric_name         = "Url5xxCount"
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  datapoints_to_alarm = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"

  dimensions = {
    FunctionName = aws_lambda_function.api.function_name
    Resource     = local.lambda_resource_dimension
  }

  alarm_actions             = var.alarm_action_arns
  insufficient_data_actions = []

  tags = local.common_tags
}

resource "aws_cloudwatch_metric_alarm" "lambda_concurrency" {
  alarm_name          = "${var.name_prefix}-api-concurrency"
  alarm_description   = "The Itinera API is using at least 80 percent of its reserved concurrency."
  namespace           = "AWS/Lambda"
  metric_name         = "ConcurrentExecutions"
  statistic           = "Maximum"
  period              = 60
  evaluation_periods  = 3
  datapoints_to_alarm = 2
  threshold           = max(1, ceil(var.lambda_reserved_concurrency * 0.8))
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"

  dimensions = {
    FunctionName = aws_lambda_function.api.function_name
  }

  alarm_actions             = var.alarm_action_arns
  insufficient_data_actions = []

  tags = local.common_tags
}

resource "aws_cloudwatch_metric_alarm" "dynamodb_throttle" {
  for_each = local.dynamodb_throttle_alarms

  alarm_name          = "${var.name_prefix}-dynamodb-${each.key}-throttles"
  alarm_description   = "The Itinera DynamoDB ${each.key} capacity throttled at least one event."
  namespace           = "AWS/DynamoDB"
  metric_name         = each.value.metric_name
  statistic           = "Sum"
  period              = 60
  evaluation_periods  = 1
  datapoints_to_alarm = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"
  dimensions          = each.value.dimensions

  alarm_actions             = var.alarm_action_arns
  insufficient_data_actions = []

  tags = local.common_tags
}
