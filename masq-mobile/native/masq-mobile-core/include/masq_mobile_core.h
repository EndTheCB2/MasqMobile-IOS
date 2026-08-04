#ifndef MASQ_MOBILE_CORE_H
#define MASQ_MOBILE_CORE_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

char *masq_mobile_get_status(void);
char *masq_mobile_configure(const char *config_json);
char *masq_mobile_import_wallet(const char *private_key);
char *masq_mobile_update_min_hops(uint8_t min_hops);
char *masq_mobile_start(void);
char *masq_mobile_stop(void);
char *masq_mobile_shutdown(void);
char *masq_mobile_reset(void);
char *masq_mobile_reset_network_profile(void);
char *masq_mobile_remove_wallet(void);
char *masq_mobile_preflight_proxy(void);
char *masq_mobile_refresh_route_proof(void);
char *masq_mobile_set_proxy_enabled(bool enabled);
char *masq_mobile_get_debt_summary(void);
char *masq_mobile_prepare_debt_settlement(void);
char *masq_mobile_confirm_debt_settlement(
    const char *quote_id,
    const char *maximum_masq_wei,
    const char *maximum_estimated_l2_fee_wei);
char *masq_mobile_get_debt_settlement_status(void);
char *masq_mobile_retry_debt_settlement(void);
void masq_mobile_string_free(char *value);

#ifdef __cplusplus
}
#endif

#endif
