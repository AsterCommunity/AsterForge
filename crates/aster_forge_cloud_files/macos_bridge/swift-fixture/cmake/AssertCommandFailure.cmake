if(NOT DEFINED COMMAND_PATH)
  message(FATAL_ERROR "COMMAND_PATH is required")
endif()

if(NOT DEFINED COMMAND_ARGUMENTS)
  set(COMMAND_ARGUMENTS "")
endif()

if(NOT DEFINED EXPECTED_REGEX)
  message(FATAL_ERROR "EXPECTED_REGEX is required")
endif()

string(REPLACE "|" ";" COMMAND_ARGUMENTS "${COMMAND_ARGUMENTS}")
execute_process(
  COMMAND "${COMMAND_PATH}" ${COMMAND_ARGUMENTS}
  RESULT_VARIABLE COMMAND_RESULT
  OUTPUT_VARIABLE COMMAND_STDOUT
  ERROR_VARIABLE COMMAND_STDERR
)
set(COMMAND_OUTPUT "${COMMAND_STDOUT}${COMMAND_STDERR}")

if(COMMAND_RESULT EQUAL 0)
  message(FATAL_ERROR "Expected the command to fail, but it exited successfully")
endif()

if(NOT COMMAND_OUTPUT MATCHES "${EXPECTED_REGEX}")
  message(FATAL_ERROR
    "Command failed without the expected output. Expected regex: ${EXPECTED_REGEX}\n"
    "Actual output:\n${COMMAND_OUTPUT}"
  )
endif()
