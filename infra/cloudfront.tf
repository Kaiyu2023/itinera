resource "aws_cloudfront_function" "edge_proof" {
  name    = "${var.name_prefix}-edge-proof"
  comment = "Reject requests that did not pass through the Itinera Cloudflare Worker"
  runtime = "cloudfront-js-2.0"
  publish = true
  code = templatefile("${path.module}/cloudfront-edge-proof.js.tftpl", {
    proof_header_json    = jsonencode(local.edge_proof_header)
    allowed_digests_json = jsonencode(var.edge_proof_sha256_hashes)
  })
}

resource "aws_cloudfront_origin_access_control" "lambda" {
  name                              = "${var.name_prefix}-lambda-oac"
  description                       = "SigV4 signing for the Itinera Lambda Function URL"
  origin_access_control_origin_type = "lambda"
  signing_behavior                  = "always"
  signing_protocol                  = "sigv4"
}

resource "aws_cloudfront_cache_policy" "api_disabled" {
  name        = "${var.name_prefix}-api-no-cache"
  comment     = "Private Itinera API responses are never shared at CloudFront"
  default_ttl = 0
  max_ttl     = 0
  min_ttl     = 0

  parameters_in_cache_key_and_forwarded_to_origin {
    enable_accept_encoding_brotli = false
    enable_accept_encoding_gzip   = false

    cookies_config {
      cookie_behavior = "none"
    }

    headers_config {
      header_behavior = "none"
    }

    query_strings_config {
      query_string_behavior = "none"
    }
  }
}

resource "aws_cloudfront_origin_request_policy" "api" {
  name    = "${var.name_prefix}-api-origin-request"
  comment = "Forward only the API metadata needed by Lambda and OAC"

  cookies_config {
    cookie_behavior = "none"
  }

  headers_config {
    header_behavior = "whitelist"
    headers {
      items = [
        "Accept",
        "Accept-Language",
        "Access-Control-Request-Headers",
        "Access-Control-Request-Method",
        "Cf-Access-Jwt-Assertion",
        "Content-Type",
        "Origin",
        "X-Amz-Content-Sha256",
      ]
    }
  }

  query_strings_config {
    query_string_behavior = "all"
  }
}

resource "aws_cloudfront_response_headers_policy" "api" {
  name    = "${var.name_prefix}-api-private"
  comment = "Keep private API responses out of browser and shared caches"

  custom_headers_config {
    items {
      header   = "Cache-Control"
      override = true
      value    = "private, no-store"
    }
  }

  security_headers_config {
    content_type_options {
      override = true
    }

    frame_options {
      frame_option = "DENY"
      override     = true
    }

    referrer_policy {
      override        = true
      referrer_policy = "no-referrer"
    }

    strict_transport_security {
      access_control_max_age_sec = 31536000
      include_subdomains         = false
      override                   = true
      preload                    = false
    }
  }
}

resource "aws_cloudfront_distribution" "api" {
  enabled         = true
  comment         = "Private Itinera API origin"
  http_version    = "http2and3"
  is_ipv6_enabled = true
  price_class     = "PriceClass_100"

  origin {
    domain_name              = local.lambda_function_url_domain
    origin_id                = local.cloudfront_origin_id
    origin_access_control_id = aws_cloudfront_origin_access_control.lambda.id
    connection_attempts      = 2
    connection_timeout       = 5

    custom_origin_config {
      http_port                = 80
      https_port               = 443
      origin_keepalive_timeout = 5
      origin_protocol_policy   = "https-only"
      origin_read_timeout      = var.lambda_timeout_seconds
      origin_ssl_protocols     = ["TLSv1.2"]
    }
  }

  default_cache_behavior {
    allowed_methods = ["DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"]
    cached_methods  = ["GET", "HEAD"]
    compress        = true

    cache_policy_id            = aws_cloudfront_cache_policy.api_disabled.id
    origin_request_policy_id   = aws_cloudfront_origin_request_policy.api.id
    response_headers_policy_id = aws_cloudfront_response_headers_policy.api.id
    target_origin_id           = local.cloudfront_origin_id
    viewer_protocol_policy     = "https-only"

    function_association {
      event_type   = "viewer-request"
      function_arn = aws_cloudfront_function.edge_proof.arn
    }
  }

  dynamic "custom_error_response" {
    for_each = toset([400, 403, 404, 405, 414, 416, 500, 501, 502, 503, 504])
    content {
      error_caching_min_ttl = 0
      error_code            = custom_error_response.value
    }
  }

  restrictions {
    geo_restriction {
      restriction_type = "none"
    }
  }

  viewer_certificate {
    cloudfront_default_certificate = true
    # AWS fixes the *.cloudfront.net default certificate to this policy. The
    # only intended viewer is the Worker, which negotiates a modern protocol.
    minimum_protocol_version = "TLSv1"
  }

  tags = merge(local.common_tags, {
    Name = "${var.name_prefix}-api"
  })
}

resource "aws_lambda_permission" "cloudfront_function_url" {
  statement_id           = "AllowCloudFrontInvokeFunctionUrl"
  action                 = "lambda:InvokeFunctionUrl"
  function_name          = aws_lambda_function.api.function_name
  qualifier              = aws_lambda_alias.live.name
  principal              = "cloudfront.amazonaws.com"
  source_arn             = aws_cloudfront_distribution.api.arn
  function_url_auth_type = "AWS_IAM"
}

resource "aws_lambda_permission" "cloudfront_invoke_function" {
  statement_id             = "AllowCloudFrontInvokeFunction"
  action                   = "lambda:InvokeFunction"
  function_name            = aws_lambda_function.api.function_name
  qualifier                = aws_lambda_alias.live.name
  principal                = "cloudfront.amazonaws.com"
  source_arn               = aws_cloudfront_distribution.api.arn
  invoked_via_function_url = true
}
