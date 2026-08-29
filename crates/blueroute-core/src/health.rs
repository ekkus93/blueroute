#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HealthLevel {
    Healthy,
    Degraded,
    Reconnecting,
    Error,
}

impl HealthLevel {
    const fn severity(self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Degraded => 1,
            Self::Reconnecting => 2,
            Self::Error => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HealthComponent {
    pub level: HealthLevel,
    pub required: bool,
}

impl HealthComponent {
    pub const fn required(level: HealthLevel) -> Self {
        Self {
            level,
            required: true,
        }
    }

    pub const fn optional(level: HealthLevel) -> Self {
        Self {
            level,
            required: false,
        }
    }
}

/// Orthogonal health components. `None` means the component is not applicable/known.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeHealth {
    pub adapter: Option<HealthComponent>,
    pub runtime_prerequisites: Option<HealthComponent>,
    pub membership: Option<HealthComponent>,
    pub links: Option<HealthComponent>,
    pub topology: Option<HealthComponent>,
    pub gateway: Option<HealthComponent>,
}

impl NodeHealth {
    pub fn aggregate(&self) -> HealthLevel {
        let mut result = HealthLevel::Healthy;
        for component in [
            self.adapter,
            self.runtime_prerequisites,
            self.membership,
            self.links,
            self.topology,
            self.gateway,
        ]
        .into_iter()
        .flatten()
        {
            if !component.required && component.level == HealthLevel::Error {
                if result.severity() < HealthLevel::Degraded.severity() {
                    result = HealthLevel::Degraded;
                }
                continue;
            }
            if component.level.severity() > result.severity() {
                result = component.level;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_optional_gateway_does_not_hurt_local_network_health() {
        let health = NodeHealth {
            adapter: Some(HealthComponent::required(HealthLevel::Healthy)),
            runtime_prerequisites: Some(HealthComponent::required(HealthLevel::Healthy)),
            membership: Some(HealthComponent::required(HealthLevel::Healthy)),
            links: Some(HealthComponent::required(HealthLevel::Healthy)),
            topology: Some(HealthComponent::required(HealthLevel::Healthy)),
            gateway: None,
        };
        assert_eq!(health.aggregate(), HealthLevel::Healthy);
    }

    #[test]
    fn optional_feature_error_degrades_instead_of_failing_node() {
        let health = NodeHealth {
            gateway: Some(HealthComponent::optional(HealthLevel::Error)),
            ..NodeHealth::default()
        };
        assert_eq!(health.aggregate(), HealthLevel::Degraded);
    }

    #[test]
    fn required_error_wins() {
        let health = NodeHealth {
            adapter: Some(HealthComponent::required(HealthLevel::Error)),
            links: Some(HealthComponent::required(HealthLevel::Reconnecting)),
            ..NodeHealth::default()
        };
        assert_eq!(health.aggregate(), HealthLevel::Error);
    }
}
