locals {
  function_name = "${var.name_prefix}-api"
  table_name    = "${var.name_prefix}-data"
  gsi_name      = "gsi1"

  cloudflare_access_team_domain = trimsuffix(
    trimspace(var.cloudflare_access_team_domain),
    "/",
  )

  common_tags = merge(var.tags, {
    Application = "Itinera"
    ManagedBy   = "Terraform"
  })
}
