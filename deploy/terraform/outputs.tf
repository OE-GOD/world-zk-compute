# World ZK Compute — Terraform Outputs

output "alb_dns_name" {
  description = "DNS name of the Application Load Balancer"
  value       = aws_lb.worldzk.dns_name
}

output "alb_zone_id" {
  description = "Route53 zone ID of the ALB (for DNS alias records)"
  value       = aws_lb.worldzk.zone_id
}

output "proof_bucket" {
  description = "S3 bucket name for immutable proof storage"
  value       = aws_s3_bucket.proofs.id
}

output "proof_bucket_arn" {
  description = "ARN of the S3 proof storage bucket"
  value       = aws_s3_bucket.proofs.arn
}

output "ecs_cluster_name" {
  description = "Name of the ECS cluster"
  value       = aws_ecs_cluster.worldzk.name
}

output "ecs_cluster_arn" {
  description = "ARN of the ECS cluster"
  value       = aws_ecs_cluster.worldzk.arn
}

output "vpc_id" {
  description = "ID of the VPC"
  value       = aws_vpc.main.id
}

output "private_subnet_ids" {
  description = "IDs of the private subnets (for additional services)"
  value       = aws_subnet.private[*].id
}

output "ecs_security_group_id" {
  description = "Security group ID for ECS services (for additional services)"
  value       = aws_security_group.ecs_services.id
}

output "efs_file_system_id" {
  description = "EFS file system ID for proof registry data"
  value       = aws_efs_file_system.proof_registry.id
}

output "cloudwatch_log_group" {
  description = "CloudWatch log group name"
  value       = aws_cloudwatch_log_group.worldzk.name
}

output "verifier_service_name" {
  description = "ECS service name for the verifier"
  value       = aws_ecs_service.verifier.name
}

output "proof_registry_service_name" {
  description = "ECS service name for the proof registry"
  value       = aws_ecs_service.proof_registry.name
}

output "proof_generator_service_name" {
  description = "ECS service name for the proof generator"
  value       = aws_ecs_service.proof_generator.name
}
