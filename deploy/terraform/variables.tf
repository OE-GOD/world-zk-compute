# World ZK Compute — Terraform Variables
#
# Required variables (no default):
#   - ecr_registry: ECR registry URL (e.g., "123456789012.dkr.ecr.us-east-1.amazonaws.com")
#
# Optional overrides for production:
#   - certificate_arn: ACM certificate ARN for HTTPS
#   - domain_name: Custom domain for the ALB
#   - environment: "production", "staging", etc.

# --------------------------------------------------------------------------
# Project Metadata
# --------------------------------------------------------------------------

variable "project_name" {
  description = "Project name used as prefix for all resources"
  type        = string
  default     = "worldzk"
}

variable "environment" {
  description = "Deployment environment (production, staging, dev)"
  type        = string
  default     = "production"
}

variable "aws_region" {
  description = "AWS region for all resources"
  type        = string
  default     = "us-east-1"
}

# --------------------------------------------------------------------------
# Container Images
# --------------------------------------------------------------------------

variable "ecr_registry" {
  description = "ECR registry URL (e.g., 123456789012.dkr.ecr.us-east-1.amazonaws.com)"
  type        = string
}

variable "image_tag" {
  description = "Docker image tag for all services"
  type        = string
  default     = "0.1.0"
}

# --------------------------------------------------------------------------
# Verifier Service
# --------------------------------------------------------------------------

variable "verifier_cpu" {
  description = "CPU units for verifier task (256, 512, 1024, 2048, 4096)"
  type        = number
  default     = 512
}

variable "verifier_memory" {
  description = "Memory (MB) for verifier task"
  type        = number
  default     = 1024
}

variable "verifier_desired_count" {
  description = "Number of verifier task instances"
  type        = number
  default     = 2
}

variable "verifier_max_count" {
  description = "Maximum number of verifier instances when autoscaling is enabled"
  type        = number
  default     = 10
}

variable "rate_limit_rpm" {
  description = "Rate limit: maximum requests per minute per client"
  type        = number
  default     = 100
}

# --------------------------------------------------------------------------
# Proof Registry Service
# --------------------------------------------------------------------------

variable "proof_registry_cpu" {
  description = "CPU units for proof registry task"
  type        = number
  default     = 512
}

variable "proof_registry_memory" {
  description = "Memory (MB) for proof registry task"
  type        = number
  default     = 1024
}

# --------------------------------------------------------------------------
# Proof Generator Service
# --------------------------------------------------------------------------

variable "proof_generator_cpu" {
  description = "CPU units for proof generator task (proving is CPU-intensive)"
  type        = number
  default     = 2048
}

variable "proof_generator_memory" {
  description = "Memory (MB) for proof generator task (proving needs significant RAM)"
  type        = number
  default     = 8192
}

variable "proof_generator_desired_count" {
  description = "Number of proof generator task instances"
  type        = number
  default     = 1
}

# --------------------------------------------------------------------------
# Storage
# --------------------------------------------------------------------------

variable "proof_retention_years" {
  description = "Number of years to retain proofs under S3 Object Lock (GOVERNANCE mode)"
  type        = number
  default     = 7
}

# --------------------------------------------------------------------------
# Networking & TLS
# --------------------------------------------------------------------------

variable "domain_name" {
  description = "Custom domain name for the ALB (optional, leave empty to use ALB DNS)"
  type        = string
  default     = ""
}

variable "certificate_arn" {
  description = "ACM certificate ARN for HTTPS (optional, leave empty for HTTP-only)"
  type        = string
  default     = ""
}

variable "enable_nat_gateway" {
  description = "Create a NAT gateway for private subnet internet access (costs ~$32/month)"
  type        = bool
  default     = false
}

# --------------------------------------------------------------------------
# Observability
# --------------------------------------------------------------------------

variable "log_retention_days" {
  description = "CloudWatch log retention in days"
  type        = number
  default     = 90
}

# --------------------------------------------------------------------------
# Scaling
# --------------------------------------------------------------------------

variable "enable_autoscaling" {
  description = "Enable CPU-based autoscaling for the verifier service"
  type        = bool
  default     = false
}
