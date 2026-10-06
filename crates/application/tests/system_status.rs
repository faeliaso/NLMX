use std::sync::Arc;

use nlmx_application::use_cases::GetSystemStatus;
use nlmx_domain::generation::LanguageModelStatus;
use nlmx_testing::{FakeLlmProvider, FakeRuntime, FakeStorage};

fn use_case(llm: Arc<FakeLlmProvider>) -> GetSystemStatus {
    GetSystemStatus::new(
        llm,
        Arc::new(FakeStorage::healthy()),
        Ok("pdfium".into()),
        Arc::new(FakeRuntime::llama()),
    )
}

#[tokio::test]
async fn recheck_asks_the_provider_again_and_sees_the_new_state() {
    let llm = Arc::new(FakeLlmProvider::new(LanguageModelStatus::LicenseRequired));
    let status = use_case(llm.clone());

    assert_eq!(
        status.language_model().await,
        LanguageModelStatus::LicenseRequired
    );
    assert_eq!(llm.recheck_calls(), 0);

    llm.set_status(LanguageModelStatus::Available);
    assert_eq!(
        status.recheck_language_model().await,
        LanguageModelStatus::Available
    );
    assert_eq!(llm.recheck_calls(), 1);
}
