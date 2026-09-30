//! 供应商无关的费用算术；不计数 token，不预留数据库额度，也不授权发送。
//!
//! 金额使用部署者指定币种的百万分之一单位，限于 `PostgreSQL` BIGINT。
//! 调用方必须冻结价格/计数器版本，并提供经适配器证明的可计费 token 上界。

/// 服务端价格：每百万 token 的微货币单位价格。禁止隐式免费价格。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenPrices {
    pub input_per_million: u64,
    pub output_per_million: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetError {
    InvalidPrices,
    InvalidTokenBounds,
    InvalidLimit,
    AmountOverflow,
    RequestLimitExceeded,
    DailyLimitExceeded,
}

/// 已计算的预留计划，不表示数据库已接受预留。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CostReservation {
    prices: TokenPrices,
    input_bound: u64,
    output_bound: u64,
    amount: i64,
}

/// 只有经适配器验证完整计费语义的 usage 才能作为结算依据。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BillableUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Settlement {
    /// 已确认用量：差额可以在原预留日退还。
    Verified { charged: i64, released: i64 },
    /// 缺失或越界用量：保留全部额度，越界必须告警并禁用该配置。
    RetainReservation { charged: i64 },
}

fn cost(prices: TokenPrices, input: u64, output: u64) -> Result<i64, BudgetError> {
    // 分别向上取整，避免输入/输出各自的计费舍入造成低估；u128 避免乘法溢出。
    let input = (u128::from(input) * u128::from(prices.input_per_million)).div_ceil(1_000_000);
    let output = (u128::from(output) * u128::from(prices.output_per_million)).div_ceil(1_000_000);
    i64::try_from(input + output).map_err(|_| BudgetError::AmountOverflow)
}

impl CostReservation {
    /// 为只有输入 token 费用的向量化调用计算预留；不虚构输出费用。
    ///
    /// # Errors
    /// 输入价格、上界和单次额度必须为正；拒绝溢出或超过额度。
    pub fn quote_input(
        input_price_per_million: u64,
        input_bound: u64,
        request_limit: i64,
    ) -> Result<Self, BudgetError> {
        if input_price_per_million == 0 {
            return Err(BudgetError::InvalidPrices);
        }
        if input_bound == 0 {
            return Err(BudgetError::InvalidTokenBounds);
        }
        if request_limit <= 0 {
            return Err(BudgetError::InvalidLimit);
        }
        let prices = TokenPrices {
            input_per_million: input_price_per_million,
            output_per_million: 0,
        };
        let amount = cost(prices, input_bound, 0)?;
        if amount > request_limit {
            return Err(BudgetError::RequestLimitExceeded);
        }
        Ok(Self {
            prices,
            input_bound,
            output_bound: 0,
            amount,
        })
    }

    /// 使用已验证的模型输入上界和硬输出上限计算保守预留。
    ///
    /// # Errors
    /// 价格、上界和请求限额必须为正；拒绝 BIGINT 溢出或超过单次额度。
    pub fn quote(
        prices: TokenPrices,
        input_bound: u64,
        output_bound: u64,
        request_limit: i64,
    ) -> Result<Self, BudgetError> {
        if prices.input_per_million == 0 || prices.output_per_million == 0 {
            return Err(BudgetError::InvalidPrices);
        }
        if input_bound == 0 || output_bound == 0 {
            return Err(BudgetError::InvalidTokenBounds);
        }
        if request_limit <= 0 {
            return Err(BudgetError::InvalidLimit);
        }
        let amount = cost(prices, input_bound, output_bound)?;
        if amount > request_limit {
            return Err(BudgetError::RequestLimitExceeded);
        }
        Ok(Self {
            prices,
            input_bound,
            output_bound,
            amount,
        })
    }

    #[must_use]
    pub const fn amount(self) -> i64 {
        self.amount
    }

    /// 检查当前已占用（预留 + 已结算）的日额度，返回新总额。
    /// 调用方必须在同一数据库事务的用户/日期锁内读取、检查并写入。
    ///
    /// # Errors
    /// 拒绝无效限额、负账本、溢出或超出日额度。
    pub fn reserve_against(self, occupied: i64, daily_limit: i64) -> Result<i64, BudgetError> {
        if occupied < 0 || daily_limit <= 0 {
            return Err(BudgetError::InvalidLimit);
        }
        let total = occupied
            .checked_add(self.amount)
            .ok_or(BudgetError::AmountOverflow)?;
        if total > daily_limit {
            return Err(BudgetError::DailyLimitExceeded);
        }
        Ok(total)
    }

    /// 超时、未知结果、派发后取消或不可信 usage 一律传 None，不退款。
    #[must_use]
    pub fn settle(self, usage: Option<BillableUsage>) -> Settlement {
        if let Some(usage) = usage
            && usage.input_tokens <= self.input_bound
            && usage.output_tokens <= self.output_bound
            && let Ok(charged) = cost(self.prices, usage.input_tokens, usage.output_tokens)
        {
            return Settlement::Verified {
                charged,
                released: self.amount - charged,
            };
        }
        Settlement::RetainReservation {
            charged: self.amount,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRICES: TokenPrices = TokenPrices {
        input_per_million: 1_500_000,
        output_per_million: 6_000_000,
    };

    #[test]
    fn rounds_each_charge_up_and_accepts_exact_request_limit() {
        let prices = TokenPrices {
            input_per_million: 1,
            output_per_million: 1,
        };
        assert_eq!(CostReservation::quote(prices, 1, 1, 2).unwrap().amount(), 2);
        assert_eq!(
            CostReservation::quote(prices, 1, 1, 1),
            Err(BudgetError::RequestLimitExceeded)
        );
        assert_eq!(
            CostReservation::quote(PRICES, 1000, 1024, 7644)
                .unwrap()
                .amount(),
            7644
        );
    }

    #[test]
    fn input_only_quote_rounds_without_output_fees_and_settles_verified_usage() {
        let quote = CostReservation::quote_input(1, 1, 1).unwrap();
        assert_eq!(quote.amount(), 1);
        assert_eq!(
            quote.settle(Some(BillableUsage {
                input_tokens: 0,
                output_tokens: 0
            })),
            Settlement::Verified {
                charged: 0,
                released: 1
            }
        );
        assert_eq!(
            quote.settle(Some(BillableUsage {
                input_tokens: 1,
                output_tokens: 0
            })),
            Settlement::Verified {
                charged: 1,
                released: 0
            }
        );
        for usage in [
            None,
            Some(BillableUsage {
                input_tokens: 2,
                output_tokens: 0,
            }),
            Some(BillableUsage {
                input_tokens: 0,
                output_tokens: 1,
            }),
        ] {
            assert_eq!(
                quote.settle(usage),
                Settlement::RetainReservation { charged: 1 }
            );
        }
    }

    #[test]
    fn input_only_quote_rejects_zero_inputs_limits_over_budget_and_overflow() {
        assert_eq!(
            CostReservation::quote_input(0, 1, 1),
            Err(BudgetError::InvalidPrices)
        );
        assert_eq!(
            CostReservation::quote_input(1, 0, 1),
            Err(BudgetError::InvalidTokenBounds)
        );
        assert_eq!(
            CostReservation::quote_input(1, 1, 0),
            Err(BudgetError::InvalidLimit)
        );
        assert_eq!(
            CostReservation::quote_input(1_000_000, 2, 1),
            Err(BudgetError::RequestLimitExceeded)
        );
        assert_eq!(
            CostReservation::quote_input(u64::MAX, u64::MAX, i64::MAX),
            Err(BudgetError::AmountOverflow)
        );
    }

    #[test]
    fn rejects_missing_prices_bounds_and_limits() {
        for prices in [
            TokenPrices {
                input_per_million: 0,
                ..PRICES
            },
            TokenPrices {
                output_per_million: 0,
                ..PRICES
            },
        ] {
            assert_eq!(
                CostReservation::quote(prices, 1, 1, 100),
                Err(BudgetError::InvalidPrices)
            );
        }
        for (input, output) in [(0, 1), (1, 0)] {
            assert_eq!(
                CostReservation::quote(PRICES, input, output, 100),
                Err(BudgetError::InvalidTokenBounds)
            );
        }
        for limit in [-1, 0] {
            assert_eq!(
                CostReservation::quote(PRICES, 1, 1, limit),
                Err(BudgetError::InvalidLimit)
            );
        }
    }

    #[test]
    fn rejects_extreme_cost_without_panicking_or_wrapping() {
        let prices = TokenPrices {
            input_per_million: u64::MAX,
            output_per_million: u64::MAX,
        };
        assert_eq!(
            CostReservation::quote(prices, u64::MAX, u64::MAX, i64::MAX),
            Err(BudgetError::AmountOverflow)
        );
    }

    #[test]
    fn enforces_remaining_daily_budget_including_outstanding_reservations() {
        let quote = CostReservation::quote(PRICES, 1000, 1024, 8000).unwrap();
        assert_eq!(quote.reserve_against(2356, 10000), Ok(10000));
        assert_eq!(
            quote.reserve_against(2357, 10000),
            Err(BudgetError::DailyLimitExceeded)
        );
        assert_eq!(
            quote.reserve_against(i64::MAX, i64::MAX),
            Err(BudgetError::AmountOverflow)
        );
        for (occupied, limit) in [(-1, 10000), (0, 0), (0, -1)] {
            assert_eq!(
                quote.reserve_against(occupied, limit),
                Err(BudgetError::InvalidLimit)
            );
        }
    }

    #[test]
    fn settles_only_complete_usage_within_frozen_bounds() {
        let quote = CostReservation::quote(PRICES, 1000, 1024, 8000).unwrap();
        assert_eq!(
            quote.settle(Some(BillableUsage {
                input_tokens: 100,
                output_tokens: 10
            })),
            Settlement::Verified {
                charged: 210,
                released: 7434
            }
        );
        assert_eq!(
            quote.settle(Some(BillableUsage {
                input_tokens: 1000,
                output_tokens: 1024
            })),
            Settlement::Verified {
                charged: 7644,
                released: 0
            }
        );
        assert_eq!(
            quote.settle(Some(BillableUsage {
                input_tokens: 0,
                output_tokens: 0
            })),
            Settlement::Verified {
                charged: 0,
                released: 7644
            }
        );
        for usage in [
            None,
            Some(BillableUsage {
                input_tokens: 1001,
                output_tokens: 0,
            }),
            Some(BillableUsage {
                input_tokens: 0,
                output_tokens: 1025,
            }),
        ] {
            assert_eq!(
                quote.settle(usage),
                Settlement::RetainReservation { charged: 7644 }
            );
        }
    }
}
