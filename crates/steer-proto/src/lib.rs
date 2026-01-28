pub mod common {
    pub mod v1 {
        #![allow(
            clippy::all,
            dead_code,
            non_camel_case_types,
            non_snake_case,
            non_upper_case_globals,
            unfulfilled_lint_expectations,
            unused_imports,
            unused_variables
        )]
        tonic::include_proto!("steer.common.v1");
    }
}

pub mod agent {
    pub mod v1 {
        #![allow(
            clippy::all,
            dead_code,
            non_camel_case_types,
            non_snake_case,
            non_upper_case_globals,
            unfulfilled_lint_expectations,
            unused_imports,
            unused_variables
        )]
        tonic::include_proto!("steer.agent.v1");
    }
}

pub mod remote_workspace {
    pub mod v1 {
        #![allow(
            clippy::all,
            dead_code,
            non_camel_case_types,
            non_snake_case,
            non_upper_case_globals,
            unfulfilled_lint_expectations,
            unused_imports,
            unused_variables
        )]
        tonic::include_proto!("steer.remote_workspace.v1");
    }
}
