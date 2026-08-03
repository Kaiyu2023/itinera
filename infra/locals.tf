locals {
  function_name        = "${var.name_prefix}-api"
  table_name           = "${var.name_prefix}-data"
  gsi_name             = "gsi1"
  cloudfront_origin_id = "${var.name_prefix}-lambda-url"
  edge_proof_header    = "x-itinera-edge-proof"

  lambda_function_url_domain = trimsuffix(
    trimprefix(aws_lambda_function_url.api.function_url, "https://"),
    "/",
  )

  cloudflare_access_team_domain = trimsuffix(
    trimspace(var.cloudflare_access_team_domain),
    "/",
  )

  common_tags = merge(var.tags, {
    Application = "Itinera"
    ManagedBy   = "Terraform"
  })
}
