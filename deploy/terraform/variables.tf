variable "project" {
  default = "world-zk-compute"
}

variable "aws_region" {
  default = "us-east-1"
}

variable "ecr_repository" {
  description = "ECR repository URL for the verifier image"
  type        = string
}

variable "image_tag" {
  default = "latest"
}

variable "verifier_cpu" {
  default = "512"
}

variable "verifier_memory" {
  default = "1024"
}
